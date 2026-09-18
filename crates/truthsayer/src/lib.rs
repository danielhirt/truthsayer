//! truthsayer: calibrated yes/no supervision for agent harnesses.
//!
//! An agent harness makes many small judgments per turn that get buried
//! inside one big generation: did that tool call fail, did the edit
//! change behavior or only formatting, is the agent looping, did it
//! touch something it was told not to, is the task done. truthsayer
//! asks those as typed questions of a decision model (TypeSafe's Jev
//! through OpenRouter today), gets calibrated probabilities back, and
//! applies rules in code. The harness branches on the result.
//!
//! ```no_run
//! use std::sync::Arc;
//! use serde_json::json;
//! use truthsayer::{Observation, OpenRouterJudge, Supervisor, ToolCall, rubric::builtin};
//!
//! # async fn run() -> Result<(), truthsayer::Error> {
//! let judge = Arc::new(OpenRouterJudge::from_env()?);
//! let sup = Supervisor::new(judge).rubrics(builtin::all());
//!
//! let obs = Observation::new("Fix the failing test in src/cache.rs")
//!     .constraint("Do not modify anything under src/auth")
//!     .tool(ToolCall::new("bash", json!({"cmd": "cargo test"}))
//!         .output("error[E0425]: cannot find value `lock` in this scope", true));
//!
//! let report = sup.supervise_with(&obs, &["tool-result", "progress"]).await?;
//! if report.halted() { /* stop the turn */ }
//! # Ok(()) }
//! ```

pub mod error;
pub mod judge;
#[cfg(feature = "openrouter")]
pub mod openrouter;
pub mod question;
pub mod rubric;
pub mod supervisor;

pub use error::Error;
pub use judge::{Judge, MockJudge};
#[cfg(feature = "openrouter")]
pub use openrouter::OpenRouterJudge;
pub use question::{Answer, Answers, Judgment, Question, Questions, State, Usage};
pub use rubric::{Action, Condition, Rubric, Rule};
pub use supervisor::{
    Finding, JsonlSink, Observation, Recommendation, Record, Report, Sink, Supervisor, ToolCall,
    Verdict, summarize,
};
