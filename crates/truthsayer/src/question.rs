//! Question and answer types. These mirror the decision-model contract
//! (TypeSafe's System One API, also exposed by OpenRouter's Decisions
//! router): three question types, one answer per question, every answer
//! constrained to the question's schema and carrying a probability.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A question sent to the judge. `instructions` and every criterion are
/// plain strings: OpenRouter's adapter rejects structured values, and
/// plain strings port to every SDK unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Question {
    /// A yes/no question. The answer is P(yes).
    Noul {
        instructions: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<NoulCriteria>,
    },
    /// Pick one option. The answer is the argmax plus a distribution.
    Choice {
        instructions: String,
        criteria: BTreeMap<String, String>,
    },
    /// Rate against ordered levels. The answer is an expected level
    /// plus a distribution.
    Score {
        instructions: String,
        criteria: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoulCriteria {
    #[serde(rename = "true")]
    pub yes: String,
    #[serde(rename = "false")]
    pub no: String,
}

impl Question {
    pub fn noul(instructions: impl Into<String>) -> Self {
        Question::Noul {
            instructions: instructions.into(),
            criteria: None,
        }
    }

    pub fn noul_with(
        instructions: impl Into<String>,
        yes: impl Into<String>,
        no: impl Into<String>,
    ) -> Self {
        Question::Noul {
            instructions: instructions.into(),
            criteria: Some(NoulCriteria {
                yes: yes.into(),
                no: no.into(),
            }),
        }
    }

    pub fn choice<I, K, V>(instructions: impl Into<String>, options: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Question::Choice {
            instructions: instructions.into(),
            criteria: options
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    pub fn score<I, S>(instructions: impl Into<String>, levels: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Question::Score {
            instructions: instructions.into(),
            criteria: levels.into_iter().map(Into::into).collect(),
        }
    }

    pub fn instructions(&self) -> &str {
        match self {
            Question::Noul { instructions, .. }
            | Question::Choice { instructions, .. }
            | Question::Score { instructions, .. } => instructions,
        }
    }
}

/// Questions keyed by an id the caller chooses. Answers come back
/// under the same ids. Ordered so requests serialize deterministically.
pub type Questions = BTreeMap<String, Question>;

/// One answer. `confidence` is the model's own statistic over the
/// distribution (1 - normalized entropy); nouls do not carry one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Answer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        #[serde(default)]
        probabilities: BTreeMap<String, f64>,
        #[serde(default)]
        confidence: f64,
    },
    Score {
        score: f64,
        #[serde(default)]
        legend: BTreeMap<String, String>,
        #[serde(default)]
        probabilities: BTreeMap<String, f64>,
        #[serde(default)]
        confidence: f64,
    },
}

impl Answer {
    /// The headline scalar: P(yes), P(chosen option), or the expected
    /// score level.
    pub fn value(&self) -> f64 {
        match self {
            Answer::Noul { noul } => *noul,
            Answer::Choice {
                choice,
                probabilities,
                ..
            } => probabilities.get(choice).copied().unwrap_or(1.0),
            Answer::Score { score, .. } => *score,
        }
    }

    /// The label a decision rule reads: `yes`/`no` for a noul at 0.5,
    /// the option for a choice, the argmax level index for a score.
    pub fn label(&self) -> String {
        match self {
            Answer::Noul { noul } => if *noul >= 0.5 { "yes" } else { "no" }.to_string(),
            Answer::Choice { choice, .. } => choice.clone(),
            Answer::Score {
                probabilities,
                score,
                ..
            } => probabilities
                .iter()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| format!("{}", score.round() as i64)),
        }
    }

    pub fn confidence(&self) -> Option<f64> {
        match self {
            Answer::Noul { .. } => None,
            Answer::Choice { confidence, .. } | Answer::Score { confidence, .. } => {
                Some(*confidence)
            }
        }
    }
}

pub type Answers = BTreeMap<String, Answer>;

/// Token usage and cost for one judge call, when the backend reports it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default)]
    pub cost_usd: f64,
}

/// What a judge returns: answers plus bookkeeping.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Judgment {
    pub model: String,
    pub answers: Answers,
    #[serde(default)]
    pub usage: Usage,
}

/// The state a request evaluates. Any JSON value; an object with named
/// parts is the recommended shape so questions can name them by path.
pub type State = Value;
