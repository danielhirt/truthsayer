//! A judge that calls a decision model over HTTP.
//!
//! Two backends speak the same wire format, `{ model, state, questions }`
//! in and `{ model, answers, usage }` out:
//!
//! - **TypeSafe** (default): `POST https://api.typesafe.ai/v1/systemone`,
//!   model `jev-latest`. Data goes to TypeSafe only.
//! - **OpenRouter**: `POST https://openrouter.ai/api/alpha/decisions`,
//!   model `~typesafe/jev-latest`. Data goes to OpenRouter, then TypeSafe.
//!
//! Aliases move when a new model ships. After you tune thresholds, pin a
//! versioned model id (for example `jev-1.13.0`) with [`HttpJudge::with_model`].

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::judge::{BoxFuture, Judge};
use crate::question::{Answers, Judgment, Questions, State, Usage};

/// Jev list price: $0.042 per million input tokens; output is free.
const PRICE_PER_INPUT_TOKEN: f64 = 0.042 / 1e6;

/// The longest `retry-after` the judge honors. A longer wait fails the
/// call instead, because a hook cannot wait that long.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    TypeSafe,
    OpenRouter,
}

impl Backend {
    pub fn default_endpoint(self) -> &'static str {
        match self {
            Backend::TypeSafe => "https://api.typesafe.ai/v1/systemone",
            Backend::OpenRouter => "https://openrouter.ai/api/alpha/decisions",
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Backend::TypeSafe => "jev-latest",
            Backend::OpenRouter => "~typesafe/jev-latest",
        }
    }

    /// The environment variable that holds this backend's key by default.
    pub fn key_env(self) -> &'static str {
        match self {
            Backend::TypeSafe => "TYPESAFE_API_KEY",
            Backend::OpenRouter => "OPENROUTER_API_KEY",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "typesafe" => Some(Backend::TypeSafe),
            "openrouter" => Some(Backend::OpenRouter),
            _ => None,
        }
    }

    /// The first backend whose key is set: TypeSafe, then OpenRouter.
    pub fn detect() -> Option<Self> {
        [Backend::TypeSafe, Backend::OpenRouter]
            .into_iter()
            .find(|b| std::env::var(b.key_env()).is_ok_and(|v| !v.is_empty()))
    }
}

#[derive(Debug, Clone)]
pub struct HttpJudge {
    client: reqwest::Client,
    backend: Backend,
    api_key: String,
    model: String,
    endpoint: String,
    max_retries: u32,
}

#[derive(Serialize)]
struct DecisionsRequest<'a> {
    model: &'a str,
    state: &'a State,
    questions: &'a Questions,
}

#[derive(Deserialize)]
struct DecisionsResponse {
    model: String,
    answers: Answers,
    #[serde(default)]
    usage: WireUsage,
}

#[derive(Deserialize, Default)]
struct WireUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    /// OpenRouter reports cost; TypeSafe does not.
    #[serde(default)]
    cost: Option<f64>,
}

impl HttpJudge {
    pub fn new(backend: Backend, api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            backend,
            api_key: api_key.into(),
            model: backend.default_model().into(),
            endpoint: backend.default_endpoint().into(),
            max_retries: 3,
        }
    }

    /// TypeSafe if `TYPESAFE_API_KEY` is set, else OpenRouter if
    /// `OPENROUTER_API_KEY` is set.
    pub fn from_env() -> Result<Self, Error> {
        let backend = Backend::detect()
            .ok_or_else(|| Error::Config("set TYPESAFE_API_KEY (or OPENROUTER_API_KEY)".into()))?;
        let key = std::env::var(backend.key_env()).unwrap_or_default();
        Ok(Self::new(backend, key))
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    pub fn with_max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    async fn call(&self, state: &State, questions: &Questions) -> Result<Judgment, Error> {
        let body = DecisionsRequest {
            model: &self.model,
            state,
            questions,
        };
        let mut attempt = 0u32;
        loop {
            let mut req = self
                .client
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .json(&body);
            if self.backend == Backend::OpenRouter {
                req = req.header("X-Title", "truthsayer");
            }
            let res = req.send().await.map_err(transport)?;
            let status = res.status();
            let retry_after = res
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|s| s.is_finite() && *s >= 0.0)
                .map(Duration::from_secs_f64);
            let text = res.text().await.map_err(transport)?;
            let retryable = matches!(status.as_u16(), 429 | 529) || status.is_server_error();
            if retryable && attempt < self.max_retries {
                attempt += 1;
                let backoff = Duration::from_millis(400 * 2u64.pow(attempt));
                let wait = match retry_after {
                    Some(w) if w > MAX_RETRY_AFTER => {
                        return Err(Error::Http {
                            status: status.as_u16(),
                            body: format!("retry-after {}s is too long", w.as_secs()),
                        });
                    }
                    Some(w) => w,
                    None => backoff,
                };
                tokio::time::sleep(wait).await;
                continue;
            }
            if !status.is_success() {
                return Err(Error::Http {
                    status: status.as_u16(),
                    body: text.chars().take(400).collect(),
                });
            }
            let parsed: DecisionsResponse = serde_json::from_str(&text)?;
            for id in questions.keys() {
                if !parsed.answers.contains_key(id) {
                    return Err(Error::MissingAnswer(id.clone()));
                }
            }
            return Ok(Judgment {
                model: parsed.model,
                answers: parsed.answers,
                usage: Usage {
                    input_tokens: parsed.usage.input_tokens,
                    output_tokens: parsed.usage.output_tokens,
                    cost_usd: parsed
                        .usage
                        .cost
                        .unwrap_or(parsed.usage.input_tokens as f64 * PRICE_PER_INPUT_TOKEN),
                },
            });
        }
    }
}

/// A transport error with its full cause chain, so a log line says
/// why the connection failed and not only that it did.
fn transport(e: reqwest::Error) -> Error {
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    Error::Transport(msg)
}

impl Judge for HttpJudge {
    fn judge<'a>(
        &'a self,
        state: &'a State,
        questions: &'a Questions,
    ) -> BoxFuture<'a, Result<Judgment, Error>> {
        Box::pin(self.call(state, questions))
    }

    fn name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    //! Tests against a local HTTP server that replays scripted responses.
    use super::*;
    use crate::question::Question;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    struct Seen {
        auth: String,
        title: Option<String>,
        body: serde_json::Value,
    }

    /// One scripted reply: status, extra headers, body.
    type Reply = (u16, Vec<(&'static str, &'static str)>, String);

    /// Serve each scripted reply once, in order.
    fn serve(script: Vec<Reply>) -> (String, Arc<Mutex<Vec<Seen>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for (status, headers, body) in script {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let (mut len, mut auth, mut title) = (0usize, String::new(), None);
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let l = line.trim_end();
                    if l.is_empty() {
                        break;
                    }
                    let lower = l.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    } else if lower.starts_with("authorization:") {
                        auth = l["authorization:".len()..].trim().to_string();
                    } else if lower.starts_with("x-title:") {
                        title = Some(l["x-title:".len()..].trim().to_string());
                    }
                }
                let mut buf = vec![0; len];
                reader.read_exact(&mut buf).unwrap();
                log.lock().unwrap().push(Seen {
                    auth,
                    title,
                    body: serde_json::from_slice(&buf).unwrap(),
                });
                let mut resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
                    body.len()
                );
                for (k, v) in headers {
                    resp.push_str(&format!("{k}: {v}\r\n"));
                }
                resp.push_str("\r\n");
                resp.push_str(&body);
                let mut s = stream;
                s.write_all(resp.as_bytes()).unwrap();
            }
        });
        (format!("http://{addr}/v1/systemone"), seen)
    }

    fn questions() -> Questions {
        let mut q = Questions::new();
        q.insert(
            "failed".into(),
            Question::noul("Does `state` show a failure?"),
        );
        q
    }

    const OK: &str = r#"{"model":"jev-1.13.0","answers":{"failed":{"type":"noul","noul":0.92}},"usage":{"input_tokens":1000,"output_tokens":20}}"#;

    #[test]
    fn backend_defaults() {
        assert_eq!(Backend::TypeSafe.default_model(), "jev-latest");
        assert_eq!(Backend::OpenRouter.default_model(), "~typesafe/jev-latest");
        assert_eq!(Backend::parse("TypeSafe"), Some(Backend::TypeSafe));
        assert_eq!(Backend::parse("other"), None);
    }

    #[tokio::test]
    async fn typesafe_request_shape_and_cost() {
        let (url, seen) = serve(vec![(200, vec![], OK.into())]);
        let judge = HttpJudge::new(Backend::TypeSafe, "k1").with_endpoint(url);
        let j = judge
            .judge(&serde_json::json!("x"), &questions())
            .await
            .unwrap();
        assert_eq!(j.model, "jev-1.13.0");
        assert!((j.usage.cost_usd - 1000.0 * PRICE_PER_INPUT_TOKEN).abs() < 1e-12);
        let seen = seen.lock().unwrap();
        assert_eq!(seen[0].auth, "Bearer k1");
        assert_eq!(seen[0].title, None);
        assert_eq!(seen[0].body["model"], "jev-latest");
        assert_eq!(seen[0].body["questions"]["failed"]["type"], "noul");
    }

    #[tokio::test]
    async fn retries_overload_and_honors_retry_after() {
        let (url, seen) = serve(vec![
            (529, vec![("retry-after", "0")], "{}".into()),
            (429, vec![("retry-after", "0.05")], "{}".into()),
            (200, vec![], OK.into()),
        ]);
        let started = std::time::Instant::now();
        let judge = HttpJudge::new(Backend::OpenRouter, "k2").with_endpoint(url);
        judge
            .judge(&serde_json::json!("x"), &questions())
            .await
            .unwrap();
        // retry-after replaces the backoff, which would be 0.8 s + 1.6 s.
        assert!(started.elapsed() < Duration::from_millis(1500));
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0].title.as_deref(), Some("truthsayer"));
    }

    #[tokio::test]
    async fn long_retry_after_fails_fast() {
        let (url, _) = serve(vec![(429, vec![("retry-after", "120")], "{}".into())]);
        let judge = HttpJudge::new(Backend::TypeSafe, "k").with_endpoint(url);
        let err = judge
            .judge(&serde_json::json!("x"), &questions())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Http { status: 429, .. }), "{err}");
    }

    #[tokio::test]
    async fn client_errors_are_not_retried() {
        let (url, seen) = serve(vec![(401, vec![], r#"{"detail":"bad key"}"#.into())]);
        let judge = HttpJudge::new(Backend::TypeSafe, "k").with_endpoint(url);
        let err = judge
            .judge(&serde_json::json!("x"), &questions())
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Http { status: 401, .. }), "{err}");
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}
