//! Secret redaction. An observation leaves the machine on every judge
//! call and lands on disk in every sink record, so credentials that
//! appear in tool input or output are replaced before either happens.
//!
//! This is a best-effort filter for common credential shapes, not a
//! guarantee. A harness that handles secrets it can name should also
//! keep them out of the observation.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

/// What replaces a redacted value.
pub const MASK: &str = "[redacted]";

static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // PEM private key blocks, whole.
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
        // Provider API keys: OpenAI, Anthropic, OpenRouter and similar `sk-` keys.
        r"\bsk-[A-Za-z0-9_\-]{20,}",
        // GitHub tokens.
        r"\bgh[pousr]_[A-Za-z0-9]{30,}",
        r"\bgithub_pat_[A-Za-z0-9_]{30,}",
        // AWS access key ids.
        r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b",
        // Slack tokens.
        r"\bxox[abprs]-[A-Za-z0-9\-]{10,}",
        // Google API keys.
        r"\bAIza[0-9A-Za-z_\-]{35}",
        // JSON web tokens.
        r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
        // Credentials embedded in URLs: scheme://user:password@host.
        r"(?i)\b[a-z][a-z0-9+.\-]*://[^\s/:@]+:[^\s/@]+@",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("redaction pattern compiles"))
    .collect()
});

/// `NAME=value`, `NAME: value`, or `"name": "value"` where the name
/// says the value is a secret. Only the value is masked.
static ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(\b[A-Z0-9_\-]*(?:SECRET|TOKEN|PASSWORD|PASSWD|API_?KEY|PRIVATE_?KEY|ACCESS_?KEY|CREDENTIAL)[A-Z0-9_\-]*"?\s*[:=]\s*"?)([^\s"',;]{4,})"#,
    )
    .expect("assignment pattern compiles")
});

/// Bearer and basic authorization header values.
static AUTH_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(\b(?:bearer|basic)\s+)([A-Za-z0-9_\-.=+/]{12,})")
        .expect("auth header pattern compiles")
});

/// Replace credential-shaped substrings in `s`.
pub fn text(s: &str) -> String {
    let mut out = s.to_string();
    for re in PATTERNS.iter() {
        if re.is_match(&out) {
            out = re.replace_all(&out, MASK).into_owned();
        }
    }
    if ASSIGNMENT.is_match(&out) {
        out = ASSIGNMENT
            .replace_all(&out, format!("${{1}}{MASK}"))
            .into_owned();
    }
    if AUTH_HEADER.is_match(&out) {
        out = AUTH_HEADER
            .replace_all(&out, format!("${{1}}{MASK}"))
            .into_owned();
    }
    out
}

/// Redact every string in a JSON value, in place. Object keys are kept.
pub fn value(v: &mut Value) {
    match v {
        Value::String(s) => {
            let r = text(s);
            if r != *s {
                *s = r;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(value),
        Value::Object(map) => map.values_mut().for_each(value),
        _ => {}
    }
}

/// True for file names that hold secrets by convention. A harness can
/// withhold the whole content of such a file instead of redacting it.
pub fn is_secret_file(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    lower == ".env"
        || (lower.starts_with(".env.")
            && !lower.ends_with(".example")
            && !lower.ends_with(".sample")
            && !lower.ends_with(".template"))
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
        || lower.starts_with("id_rsa")
        || lower.starts_with("id_ed25519")
        || lower.starts_with("id_ecdsa")
        || lower == ".netrc"
        || lower == ".pgpass"
        || lower == "credentials"
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn masks_provider_keys_and_tokens() {
        let s =
            "key sk-or-v1-0123456789abcdef0123456789 and ghp_0123456789abcdefghijklmnopqrstuvwxyz";
        let r = text(s);
        assert!(!r.contains("0123456789abcdef0123"), "{r}");
        assert!(!r.contains("ghp_"), "{r}");
        assert_eq!(r.matches(MASK).count(), 2);
    }

    #[test]
    fn masks_assignment_values_and_keeps_names() {
        let r = text("OPENROUTER_API_KEY=abcd1234efgh\nDB_PASSWORD: hunter22\nPORT=8080");
        assert!(r.contains("OPENROUTER_API_KEY=[redacted]"), "{r}");
        assert!(r.contains("DB_PASSWORD: [redacted]"), "{r}");
        assert!(r.contains("PORT=8080"), "{r}");
    }

    #[test]
    fn masks_private_keys_urls_and_bearer_headers() {
        let pem =
            "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\nBBBB\n-----END OPENSSH PRIVATE KEY-----";
        assert_eq!(text(pem), MASK);
        let r = text("postgres://app:s3cretpw@db.internal/app");
        assert!(!r.contains("s3cretpw"), "{r}");
        let r = text("Authorization: Bearer abcdefghijklmnop123");
        assert!(!r.contains("abcdefghijklmnop123"), "{r}");
    }

    #[test]
    fn leaves_ordinary_text_alone() {
        let s = "test cache::evict ... FAILED\nerror[E0425]: cannot find value `lock`";
        assert_eq!(text(s), s);
    }

    #[test]
    fn walks_json_values() {
        let mut v = json!({"cmd": "export API_KEY=abcdef123456", "n": 3, "list": ["sk-ant-0123456789abcdefghijklmn"]});
        value(&mut v);
        assert_eq!(v["cmd"], "export API_KEY=[redacted]");
        assert_eq!(v["list"][0], MASK);
        assert_eq!(v["n"], 3);
    }

    #[test]
    fn recognizes_secret_files() {
        for p in [
            ".env",
            "app/.env.local",
            "certs/server.pem",
            "/home/u/.ssh/id_ed25519",
        ] {
            assert!(is_secret_file(p), "{p}");
        }
        for p in [".env.example", "src/env.rs", "README.md", "keys.rs"] {
            assert!(!is_secret_file(p), "{p}");
        }
    }
}
