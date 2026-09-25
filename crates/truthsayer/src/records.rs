//! Record files: the JSONL sink, size-based rotation, and stable
//! record ids.
//!
//! A record file is append-only. When it reaches its size limit, it is
//! renamed to `<stem>-<unix ms>-<pid>.<ext>` in the same directory and a
//! new file starts. The name is unique per process, so two processes
//! that rotate at the same moment never overwrite each other's data.
//! The oldest rotated files past the keep count are deleted.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::supervisor::{Record, Sink};

/// Appends one JSON object per line. Creates the parent directory if
/// needed and, on Unix, creates files readable by the owner only,
/// because records hold tool output. A write failure goes to stderr and
/// never fails the supervise call.
pub struct JsonlSink {
    path: PathBuf,
    rotation: Option<(u64, usize)>,
}

impl JsonlSink {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            rotation: None,
        }
    }

    /// Rotate the file when it reaches `max_bytes`; keep `keep` rotated
    /// files.
    pub fn rotate(mut self, max_bytes: u64, keep: usize) -> Self {
        self.rotation = Some((max_bytes, keep));
        self
    }

    fn append(&self, record: &Record) -> io::Result<()> {
        if let Some((max, keep)) = self.rotation {
            rotate_if_needed(&self.path, max, keep)?;
        }
        let mut line = serde_json::to_string(record)?;
        line.push('\n');
        open_private_append(&self.path)?.write_all(line.as_bytes())
    }
}

impl Sink for JsonlSink {
    fn record(&self, record: &Record) {
        if let Err(e) = self.append(record) {
            eprintln!(
                "truthsayer: cannot write record to {}: {e}",
                self.path.display()
            );
        }
    }
}

/// Open `path` for appending, creating it and its directory as needed.
/// On Unix a new file gets mode 0600.
pub fn open_private_append(path: &Path) -> io::Result<std::fs::File> {
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// Rename `path` aside if it is `max_bytes` or larger, then delete the
/// oldest rotated files beyond `keep`. A missing file is not an error.
pub fn rotate_if_needed(path: &Path, max_bytes: u64, keep: usize) -> io::Result<()> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() >= max_bytes => {}
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    let (stem, ext) = split_name(path);
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let aside = path.with_file_name(format!("{stem}-{ms}-{}{ext}", std::process::id()));
    match std::fs::rename(path, &aside) {
        Ok(()) => {}
        // Another process rotated it first.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let rotated = rotated_files(path);
    for old in rotated.iter().take(rotated.len().saturating_sub(keep)) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

/// Every record file for `path`: rotated files oldest first, then
/// `path` itself if it exists.
pub fn record_files(path: &Path) -> Vec<PathBuf> {
    let mut files = rotated_files(path);
    if path.exists() {
        files.push(path.to_path_buf());
    }
    files
}

fn split_name(path: &Path) -> (String, String) {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (stem, ext)
}

/// Rotated files for `path`, oldest first.
fn rotated_files(path: &Path) -> Vec<PathBuf> {
    let (stem, ext) = split_name(path);
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let prefix = format!("{stem}-");
    let mut found: Vec<(u128, u32, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let middle = name.strip_prefix(&prefix)?.strip_suffix(ext.as_str())?;
            let (ms, pid) = middle.split_once('-')?;
            Some((ms.parse().ok()?, pid.parse().ok()?, e.path()))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, _, p)| p).collect()
}

/// A stable id for one record line: FNV-1a over its bytes, in hex.
/// Labels refer to records by this id.
pub fn record_id(line: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in line.trim_end().as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("truthsayer-rec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rotates_and_keeps_the_newest() {
        let d = tmp("rotate");
        let p = d.join("records.jsonl");
        for i in 0..5 {
            std::fs::write(&p, format!("{i}\n").repeat(10)).unwrap();
            rotate_if_needed(&p, 5, 2).unwrap();
            assert!(!p.exists());
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let files = record_files(&p);
        assert_eq!(files.len(), 2, "{files:?}");
        let last = std::fs::read_to_string(&files[1]).unwrap();
        assert!(last.starts_with('4'), "{last}");
        // Unrelated files in the directory are untouched.
        std::fs::write(d.join("records-notes.jsonl"), "x").unwrap();
        assert_eq!(record_files(&p).len(), 2);
    }

    #[test]
    fn small_or_missing_files_are_left_alone() {
        let d = tmp("small");
        let p = d.join("records.jsonl");
        rotate_if_needed(&p, 5, 2).unwrap();
        std::fs::write(&p, "ab").unwrap();
        rotate_if_needed(&p, 5, 2).unwrap();
        assert_eq!(record_files(&p), vec![p.clone()]);
    }

    #[test]
    fn record_ids_are_stable_and_ignore_the_newline() {
        assert_eq!(record_id("{\"a\":1}"), record_id("{\"a\":1}\n"));
        assert_ne!(record_id("{\"a\":1}"), record_id("{\"a\":2}"));
        assert_eq!(record_id("").len(), 16);
    }

    #[cfg(unix)]
    #[test]
    fn new_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp("mode");
        let p = d.join("sub").join("hook.log");
        open_private_append(&p).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
