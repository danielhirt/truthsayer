//! Rubrics: a named set of questions plus the rules that turn answers
//! into findings. Rubrics are data (`rubrics/*.json` at the repo root)
//! so every SDK reads the same files and a tuned threshold ports
//! without a code change.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::question::{Answer, Questions};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rubric {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub description: String,
    /// The state paths the questions reference, for documentation and
    /// for a harness to check it supplies them.
    #[serde(default)]
    pub state: Vec<String>,
    pub questions: Questions,
    /// A noul inside this band is `Uncertain` rather than yes or no.
    #[serde(default = "default_uncertain")]
    pub uncertain: [f64; 2],
    #[serde(default)]
    pub rules: Vec<Rule>,
}

fn default_version() -> u32 {
    1
}
fn default_uncertain() -> [f64; 2] {
    [0.3, 0.7]
}

/// What the supervisor should do when a rule fires. Ordered by
/// severity; a report's recommendation is the most severe fired action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Record a finding; do not change the recommendation.
    Note,
    /// Continue, but surface the finding.
    Warn,
    /// Stop and ask a human.
    Escalate,
    /// Stop the turn.
    Halt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub question: String,
    pub when: Condition,
    pub then: Action,
    #[serde(default)]
    pub reason: String,
}

/// When a rule fires. Exactly one field is set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    /// Headline value at or above this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_least: Option<f64>,
    /// Headline value at or below this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at_most: Option<f64>,
    /// A noul inside the rubric's uncertain band.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertain: Option<bool>,
    /// The answer's label equals this (a choice option or a score level index).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is: Option<String>,
    /// A choice or score whose confidence is below this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence_below: Option<f64>,
}

impl Condition {
    pub fn holds(&self, answer: &Answer, uncertain: [f64; 2]) -> bool {
        let v = answer.value();
        if let Some(t) = self.at_least {
            return v >= t;
        }
        if let Some(t) = self.at_most {
            return v <= t;
        }
        if let Some(want) = self.uncertain {
            let inside =
                matches!(answer, Answer::Noul { .. }) && v > uncertain[0] && v < uncertain[1];
            return inside == want;
        }
        if let Some(label) = &self.is {
            return &answer.label() == label;
        }
        if let Some(t) = self.confidence_below {
            return answer.confidence().is_some_and(|c| c < t);
        }
        false
    }
}

impl Rubric {
    pub fn from_json(json: &str) -> Result<Self, Error> {
        let rubric: Rubric = serde_json::from_str(json)?;
        rubric.validate()?;
        Ok(rubric)
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::from_json(&std::fs::read_to_string(path)?)
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.questions.is_empty() {
            return Err(Error::InvalidRubric {
                rubric: self.name.clone(),
                reason: "no questions".into(),
            });
        }
        if self.uncertain[0] >= self.uncertain[1] {
            return Err(Error::InvalidRubric {
                rubric: self.name.clone(),
                reason: "uncertain band is empty".into(),
            });
        }
        for rule in &self.rules {
            if !self.questions.contains_key(&rule.question) {
                return Err(Error::UnknownQuestion(format!(
                    "{}.{}",
                    self.name, rule.question
                )));
            }
            let set = [
                rule.when.at_least.is_some(),
                rule.when.at_most.is_some(),
                rule.when.uncertain.is_some(),
                rule.when.is.is_some(),
                rule.when.confidence_below.is_some(),
            ]
            .iter()
            .filter(|b| **b)
            .count();
            if set != 1 {
                return Err(Error::InvalidRubric {
                    rubric: self.name.clone(),
                    reason: format!("rule on `{}` must set exactly one condition", rule.question),
                });
            }
        }
        Ok(())
    }
}

/// The rubrics shipped in this repo, compiled in so a crate user needs
/// no files on disk. The JSON at `rubrics/` is the source of truth.
pub mod builtin {
    use super::Rubric;

    macro_rules! builtin {
        ($fn:ident, $file:literal) => {
            pub fn $fn() -> Rubric {
                Rubric::from_json(include_str!(concat!("../../../rubrics/", $file)))
                    .expect(concat!("built-in rubric ", $file, " is valid"))
            }
        };
    }

    builtin!(tool_result, "tool-result.json");
    builtin!(edit, "edit.json");
    builtin!(progress, "progress.json");
    builtin!(turn_end, "turn-end.json");
    builtin!(model_tier, "model-tier.json");

    /// Every built-in rubric.
    pub fn all() -> Vec<Rubric> {
        vec![tool_result(), edit(), progress(), turn_end(), model_tier()]
    }
}
