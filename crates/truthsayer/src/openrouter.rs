//! A judge backed by OpenRouter's Decisions router, which fronts
//! TypeSafe's System One models (Jev).
//!
//! Endpoint: `POST https://openrouter.ai/api/alpha/decisions` with
//! `{ model, state, questions }`. Model ids carry a tilde for the
//! "latest" alias: `~typesafe/jev-latest`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::judge::{BoxFuture, Judge};
use crate::question::{Answers, Judgment, Questions, State, Usage};

pub const DEFAULT_MODEL: &str = "~typesafe/jev-latest";
pub const DEFAULT_ENDPOINT: &str = "https://openrouter.ai/api/alpha/decisions";

#[derive(Debug, Clone)]
pub struct OpenRouterJudge {
    client: reqwest::Client,
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
    #[serde(default)]
    cost: Option<f64>,
}

impl OpenRouterJudge {
    /// Reads `OPENROUTER_API_KEY` from the environment.
    pub fn from_env() -> Result<Self, Error> {
        let key = std::env::var("OPENROUTER_API_KEY")
            .map_err(|_| Error::Config("OPENROUTER_API_KEY is not set".into()))?;
        Ok(Self::new(key))
    }

    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            api_key: api_key.into(),
            model: DEFAULT_MODEL.into(),
            endpoint: DEFAULT_ENDPOINT.into(),
            max_retries: 3,
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
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
            let res = self
                .client
                .post(&self.endpoint)
                .bearer_auth(&self.api_key)
                .header("X-Title", "truthsayer")
                .json(&body)
                .send()
                .await
                .map_err(|e| Error::Transport(e.to_string()))?;
            let status = res.status();
            let text = res
                .text()
                .await
                .map_err(|e| Error::Transport(e.to_string()))?;
            let retryable =
                status.as_u16() == 429 || status.as_u16() == 529 || status.is_server_error();
            if retryable && attempt < self.max_retries {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(400 * 2u64.pow(attempt))).await;
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
                    // Jev list price: $0.042 per million input tokens, output free.
                    cost_usd: parsed
                        .usage
                        .cost
                        .unwrap_or(parsed.usage.input_tokens as f64 * 0.042 / 1e6),
                },
            });
        }
    }
}

impl Judge for OpenRouterJudge {
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
