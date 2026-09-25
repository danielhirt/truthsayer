//! Reads records, and reads and writes the truth file.
//!
//! The truth file (`truth.jsonl`, next to the record file) holds the
//! answers a person gave with `truthsayer label`. Each line names a
//! record by id, a question as `rubric.question`, and the true answer.
//! The newest line for a record and question wins.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use truthsayer::Record;
use truthsayer::records::{open_private_append, record_files, record_id};

pub struct Loaded {
    pub id: String,
    pub record: Record,
}

/// Every record in `path` and its rotated files, oldest first. Lines
/// that do not parse are counted and skipped.
pub fn load_records(path: &Path) -> (Vec<Loaded>, usize) {
    let mut out = Vec::new();
    let mut bad = 0;
    for file in record_files(path) {
        let Ok(f) = std::fs::File::open(&file) else {
            continue;
        };
        for line in BufReader::new(f).lines() {
            let Ok(line) = line else {
                bad += 1;
                continue;
            };
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Record>(&line) {
                Ok(record) => out.push(Loaded {
                    id: record_id(&line),
                    record,
                }),
                Err(_) => bad += 1,
            }
        }
    }
    (out, bad)
}

/// The truth file that belongs to a record file.
pub fn truth_path(records: &Path) -> PathBuf {
    records.with_file_name("truth.jsonl")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Truth {
    pub id: String,
    /// `rubric.question`.
    pub question: String,
    /// `yes` or `no` for a noul; the option or level index otherwise.
    pub truth: String,
    pub at_unix_ms: u128,
}

/// Truth keyed by `(record id, question)`, newest line winning.
pub type TruthMap = HashMap<(String, String), String>;

pub fn load_truth(path: &Path) -> TruthMap {
    let mut map = TruthMap::new();
    let Ok(f) = std::fs::File::open(path) else {
        return map;
    };
    for line in BufReader::new(f).lines().map_while(Result::ok) {
        if let Ok(t) = serde_json::from_str::<Truth>(&line) {
            map.insert((t.id, t.question), t.truth);
        }
    }
    map
}

pub fn append_truth(path: &Path, t: &Truth) -> std::io::Result<()> {
    let mut line = serde_json::to_string(t)?;
    line.push('\n');
    open_private_append(path)?.write_all(line.as_bytes())
}

/// The held-out split: about 30% of records, chosen by id so the split
/// never changes between runs.
pub fn is_holdout(id: &str) -> bool {
    u64::from_str_radix(id, 16).is_ok_and(|h| h % 10 < 3)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Tune,
    Holdout,
    All,
}

impl Split {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "tune" => Some(Split::Tune),
            "holdout" => Some(Split::Holdout),
            "all" => Some(Split::All),
            _ => None,
        }
    }

    pub fn contains(self, id: &str) -> bool {
        match self {
            Split::Tune => !is_holdout(id),
            Split::Holdout => is_holdout(id),
            Split::All => true,
        }
    }
}

/// The value at a dotted path such as `tool.output` in a record state.
pub fn state_path<'a>(state: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(state, |v, key| v.get(key))
}

/// Text that is safe to print to a terminal. Records hold tool output,
/// which can contain escape sequences that rewrite the screen, so every
/// control character except newline and tab is shown as an escape.
pub fn printable(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() && c != '\n' && c != '\t' {
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn printable_escapes_terminal_controls() {
        assert_eq!(printable("a\x1b[2Jb\nc"), "a\\u{1b}[2Jb\nc");
    }

    #[test]
    fn state_paths_resolve() {
        let s = json!({"tool": {"output": "x"}});
        assert_eq!(state_path(&s, "tool.output"), Some(&json!("x")));
        assert_eq!(state_path(&s, "tool.nope"), None);
    }

    #[test]
    fn holdout_is_stable_and_near_thirty_percent() {
        let n = (0..1000u64)
            .filter(|i| is_holdout(&format!("{:016x}", i.wrapping_mul(0x9e37_79b9_7f4a_7c15))))
            .count();
        assert!((250..350).contains(&n), "{n}");
        assert!(!is_holdout("not-hex"));
    }

    #[test]
    fn newest_truth_wins() {
        let d = std::env::temp_dir().join(format!("truthsayer-truth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("truth.jsonl");
        for v in ["yes", "no"] {
            append_truth(
                &p,
                &Truth {
                    id: "a".into(),
                    question: "q.x".into(),
                    truth: v.into(),
                    at_unix_ms: 0,
                },
            )
            .unwrap();
        }
        let m = load_truth(&p);
        assert_eq!(m[&("a".to_string(), "q.x".to_string())], "no");
    }
}
