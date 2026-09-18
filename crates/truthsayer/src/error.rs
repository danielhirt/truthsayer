use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("judge returned no answer for question `{0}`")]
    MissingAnswer(String),
    #[error("rubric `{rubric}` is invalid: {reason}")]
    InvalidRubric { rubric: String, reason: String },
    #[error("rubric rule references unknown question `{0}`")]
    UnknownQuestion(String),
    #[error("{0}")]
    Config(String),
    #[error("backend HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("backend transport: {0}")]
    Transport(String),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
