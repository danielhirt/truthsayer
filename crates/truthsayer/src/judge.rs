//! The judge: anything that answers typed questions about a state.
//! The supervisor only ever talks to this trait, so the decision model
//! behind it can change without touching rubrics or policy.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use crate::error::Error;
use crate::question::{Answer, Answers, Judgment, Question, Questions, State};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait Judge: Send + Sync {
    /// Answer every question about `state`. Implementations must return
    /// one answer per question id or an error; a partial answer set is
    /// an error.
    fn judge<'a>(
        &'a self,
        state: &'a State,
        questions: &'a Questions,
    ) -> BoxFuture<'a, Result<Judgment, Error>>;

    /// A short name for records and reports.
    fn name(&self) -> &str;
}

/// A scripted judge for tests and offline runs. Answers are looked up
/// by question id; a question with no scripted answer gets a neutral
/// answer (0.5 for nouls, the first option for choices, level 0 for
/// scores) so a rubric never fails to evaluate.
#[derive(Debug, Default)]
pub struct MockJudge {
    scripted: Mutex<BTreeMap<String, Answer>>,
    calls: Mutex<Vec<(State, Questions)>>,
}

impl MockJudge {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, id: impl Into<String>, answer: Answer) -> Self {
        self.set(id, answer);
        self
    }

    pub fn noul(self, id: impl Into<String>, p: f64) -> Self {
        self.with(id, Answer::Noul { noul: p })
    }

    pub fn set(&mut self, id: impl Into<String>, answer: Answer) {
        self.scripted.lock().unwrap().insert(id.into(), answer);
    }

    /// Every `(state, questions)` pair this judge has been asked.
    pub fn calls(&self) -> Vec<(State, Questions)> {
        self.calls.lock().unwrap().clone()
    }

    fn neutral(q: &Question) -> Answer {
        match q {
            Question::Noul { .. } => Answer::Noul { noul: 0.5 },
            Question::Choice { criteria, .. } => {
                let n = criteria.len().max(1) as f64;
                let probabilities: BTreeMap<String, f64> =
                    criteria.keys().map(|k| (k.clone(), 1.0 / n)).collect();
                Answer::Choice {
                    choice: criteria.keys().next().cloned().unwrap_or_default(),
                    probabilities,
                    confidence: 0.0,
                }
            }
            Question::Score { criteria, .. } => {
                let legend = criteria
                    .iter()
                    .enumerate()
                    .map(|(i, l)| (i.to_string(), l.clone()))
                    .collect();
                let mut probabilities = BTreeMap::new();
                probabilities.insert("0".to_string(), 1.0);
                Answer::Score {
                    score: 0.0,
                    legend,
                    probabilities,
                    confidence: 1.0,
                }
            }
        }
    }
}

impl Judge for MockJudge {
    fn judge<'a>(
        &'a self,
        state: &'a State,
        questions: &'a Questions,
    ) -> BoxFuture<'a, Result<Judgment, Error>> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push((state.clone(), questions.clone()));
            let scripted = self.scripted.lock().unwrap();
            let answers: Answers = questions
                .iter()
                .map(|(id, q)| {
                    (
                        id.clone(),
                        scripted
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| Self::neutral(q)),
                    )
                })
                .collect();
            Ok(Judgment {
                model: "mock".into(),
                answers,
                usage: Default::default(),
            })
        })
    }

    fn name(&self) -> &str {
        "mock"
    }
}
