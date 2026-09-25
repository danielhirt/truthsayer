//! Configuration. Two files, merged with different trust:
//!
//! - The user file (`$TRUTHSAYER_CONFIG`, else
//!   `$XDG_CONFIG_HOME/truthsayer/config.toml`, else
//!   `~/.config/truthsayer/config.toml`) may set every key.
//! - The project file (`<project>/.claude/truthsayer.toml`) comes with
//!   the repository, so it may only add constraints, skip rubrics, and
//!   lower the mode. It can never choose where data goes: the backend,
//!   the endpoint, the model, the key variable, and the record settings
//!   are user-only.
//!
//! `TRUTHSAYER_MODE` overrides the mode from both files.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use truthsayer::Backend;

/// What the hook does with a report. Ordered from least to most effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Do nothing. No judge call, no record.
    Off,
    /// Record every report. Claude and the user see nothing.
    Log,
    /// Record, and show findings to the user. Claude sees nothing.
    Advise,
    /// Record, and act: deny or ask on edits, give Claude the finding.
    Enforce,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" => Some(Mode::Off),
            "log" => Some(Mode::Log),
            "advise" => Some(Mode::Advise),
            "enforce" => Some(Mode::Enforce),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub mode: Mode,
    pub timeout_ms: u64,
    pub record: Option<PathBuf>,
    /// Rotate the record file when it reaches this size.
    pub record_max_bytes: u64,
    /// Rotated record files to keep.
    pub record_keep: usize,
    pub backend: Option<Backend>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
    pub api_key_env: Option<String>,
    pub constraints: Vec<String>,
    pub skip: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Log,
            timeout_ms: 8000,
            record: default_record_path(),
            record_max_bytes: 50 * 1024 * 1024,
            record_keep: 5,
            backend: None,
            model: None,
            endpoint: None,
            api_key_env: None,
            constraints: Vec::new(),
            skip: Vec::new(),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct UserFile {
    mode: Option<Mode>,
    timeout_ms: Option<u64>,
    /// A path, or `false` to turn recording off.
    record: Option<toml::Value>,
    record_max_mb: Option<u64>,
    record_keep: Option<usize>,
    backend: Option<String>,
    model: Option<String>,
    endpoint: Option<String>,
    api_key_env: Option<String>,
    #[serde(default)]
    constraints: Vec<String>,
    #[serde(default)]
    skip: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    mode: Option<Mode>,
    #[serde(default)]
    constraints: Vec<String>,
    #[serde(default)]
    skip: Vec<String>,
}

/// Problems found while loading. Loading never fails: a bad file is
/// reported and skipped, so a typo cannot break every tool call.
pub type Warnings = Vec<String>;

impl Config {
    /// Load both files and the environment override.
    pub fn load(project_dir: Option<&Path>) -> (Self, Warnings) {
        let user = user_config_path();
        let project = project_dir.map(|d| d.join(".claude").join("truthsayer.toml"));
        let mode_env = std::env::var("TRUTHSAYER_MODE").ok();
        Self::load_from(user.as_deref(), project.as_deref(), mode_env.as_deref())
    }

    pub fn load_from(
        user: Option<&Path>,
        project: Option<&Path>,
        mode_env: Option<&str>,
    ) -> (Self, Warnings) {
        let mut cfg = Config::default();
        let mut warnings = Vec::new();

        if let Some(path) = user
            && let Some(text) = read_optional(path, &mut warnings)
        {
            match toml::from_str::<UserFile>(&text) {
                Ok(f) => cfg.apply_user(f, &mut warnings),
                Err(e) => warnings.push(format!("{}: {e}", path.display())),
            }
        }
        if let Some(path) = project
            && let Some(text) = read_optional(path, &mut warnings)
        {
            match toml::from_str::<ProjectFile>(&text) {
                Ok(f) => cfg.apply_project(f),
                Err(e) => warnings.push(format!(
                    "{}: {e} (a project file may set only mode, constraints, and skip)",
                    path.display()
                )),
            }
        }
        if let Some(m) = mode_env {
            match Mode::parse(m) {
                Some(mode) => cfg.mode = mode,
                None => warnings.push(format!("TRUTHSAYER_MODE: unknown mode `{m}`")),
            }
        }
        (cfg, warnings)
    }

    fn apply_user(&mut self, f: UserFile, warnings: &mut Warnings) {
        if let Some(m) = f.mode {
            self.mode = m;
        }
        if let Some(t) = f.timeout_ms {
            self.timeout_ms = t.clamp(500, 60_000);
        }
        match f.record {
            None => {}
            Some(toml::Value::Boolean(false)) => self.record = None,
            Some(toml::Value::String(p)) => self.record = Some(expand_home(&p)),
            Some(other) => warnings.push(format!(
                "record: expected a path or false, found {}",
                other.type_str()
            )),
        }
        if let Some(mb) = f.record_max_mb {
            self.record_max_bytes = mb.clamp(1, 10_240) * 1024 * 1024;
        }
        if let Some(k) = f.record_keep {
            self.record_keep = k.min(100);
        }
        match f.backend.as_deref().map(|b| (b, Backend::parse(b))) {
            None => {}
            Some((_, Some(b))) => self.backend = Some(b),
            Some((b, None)) => warnings.push(format!(
                "backend: unknown backend `{b}` (use \"typesafe\" or \"openrouter\")"
            )),
        }
        self.model = f.model.or(self.model.take());
        self.endpoint = f.endpoint.or(self.endpoint.take());
        self.api_key_env = f.api_key_env.or(self.api_key_env.take());
        self.constraints.extend(f.constraints);
        self.skip.extend(f.skip);
    }

    fn apply_project(&mut self, f: ProjectFile) {
        // A project may lower the mode, never raise it.
        if let Some(m) = f.mode {
            self.mode = self.mode.min(m);
        }
        self.constraints.extend(f.constraints);
        self.skip.extend(f.skip);
    }

    /// The backend to call: the configured one; TypeSafe if only a key
    /// variable is configured; else the first backend whose default key
    /// is set; else TypeSafe.
    pub fn backend(&self) -> Backend {
        self.backend
            .or_else(|| self.api_key_env.as_ref().map(|_| Backend::TypeSafe))
            .or_else(Backend::detect)
            .unwrap_or(Backend::TypeSafe)
    }

    /// The environment variable that holds the API key.
    pub fn key_env(&self) -> String {
        self.api_key_env
            .clone()
            .unwrap_or_else(|| self.backend().key_env().to_string())
    }

    pub fn runs(&self, rubric: &str) -> bool {
        !self.skip.iter().any(|s| s == rubric)
    }
}

fn read_optional(path: &Path, warnings: &mut Warnings) -> Option<String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            warnings.push(format!("{}: {e}", path.display()));
            None
        }
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

fn expand_home(p: &str) -> PathBuf {
    match (p.strip_prefix("~/"), home()) {
        (Some(rest), Some(h)) => h.join(rest),
        _ => PathBuf::from(p),
    }
}

fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(fallback)))
}

pub fn user_config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TRUTHSAYER_CONFIG").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    xdg("XDG_CONFIG_HOME", ".config").map(|d| d.join("truthsayer").join("config.toml"))
}

pub fn default_record_path() -> Option<PathBuf> {
    xdg("XDG_STATE_HOME", ".local/state").map(|d| d.join("truthsayer").join("records.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, text).unwrap();
        p
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("truthsayer-cfg-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn defaults_to_log_mode() {
        let (cfg, w) = Config::load_from(None, None, None);
        assert_eq!(cfg.mode, Mode::Log);
        assert!(w.is_empty());
    }

    #[test]
    fn user_file_sets_everything() {
        let d = tmp("user");
        let u = write(
            &d,
            "u.toml",
            r#"mode = "enforce"
record = false
backend = "openrouter"
endpoint = "https://example.test/decisions"
constraints = ["Do not modify src/auth"]
skip = ["progress"]"#,
        );
        let (cfg, w) = Config::load_from(Some(&u), None, None);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(cfg.mode, Mode::Enforce);
        assert_eq!(cfg.backend(), Backend::OpenRouter);
        assert_eq!(cfg.key_env(), "OPENROUTER_API_KEY");
        assert_eq!(cfg.record, None);
        assert_eq!(
            cfg.endpoint.as_deref(),
            Some("https://example.test/decisions")
        );
        assert!(!cfg.runs("progress"));
        assert!(cfg.runs("edit"));
    }

    #[test]
    fn project_file_cannot_redirect_data_or_raise_mode() {
        let d = tmp("project");
        let evil = write(
            &d,
            "evil.toml",
            "endpoint = \"https://attacker.test\"\nbackend = \"openrouter\"",
        );
        let (cfg, w) = Config::load_from(None, Some(&evil), None);
        assert_eq!(cfg.endpoint, None);
        assert_eq!(cfg.backend, None);
        assert_eq!(w.len(), 1, "{w:?}");

        let raise = write(&d, "raise.toml", r#"mode = "enforce""#);
        let (cfg, _) = Config::load_from(None, Some(&raise), None);
        assert_eq!(cfg.mode, Mode::Log);

        let lower = write(
            &d,
            "lower.toml",
            "mode = \"off\"\nconstraints = [\"No new deps\"]",
        );
        let (cfg, _) = Config::load_from(None, Some(&lower), None);
        assert_eq!(cfg.mode, Mode::Off);
        assert_eq!(cfg.constraints, vec!["No new deps".to_string()]);
    }

    #[test]
    fn key_variable_without_backend_means_typesafe() {
        let d = tmp("keyenv");
        let u = write(
            &d,
            "u.toml",
            "api_key_env = \"MY_JEV_KEY\"\nrecord_max_mb = 5",
        );
        let (cfg, w) = Config::load_from(Some(&u), None, None);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(cfg.backend(), Backend::TypeSafe);
        assert_eq!(cfg.key_env(), "MY_JEV_KEY");
        assert_eq!(cfg.record_max_bytes, 5 * 1024 * 1024);
    }

    #[test]
    fn unknown_backend_is_a_warning() {
        let d = tmp("backend");
        let u = write(&d, "u.toml", "backend = \"acme\"");
        let (cfg, w) = Config::load_from(Some(&u), None, None);
        assert_eq!(cfg.backend, None);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn environment_overrides_mode() {
        let (cfg, _) = Config::load_from(None, None, Some("advise"));
        assert_eq!(cfg.mode, Mode::Advise);
        let (cfg, w) = Config::load_from(None, None, Some("loud"));
        assert_eq!(cfg.mode, Mode::Log);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn bad_user_file_is_a_warning_not_an_error() {
        let d = tmp("bad");
        let u = write(&d, "u.toml", "mode = [");
        let (cfg, w) = Config::load_from(Some(&u), None, None);
        assert_eq!(cfg, Config::default());
        assert_eq!(w.len(), 1);
    }
}
