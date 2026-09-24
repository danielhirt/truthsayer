//! The supervisor: builds one fan-out call from every applicable
//! rubric, asks the judge, applies the rules in code, and returns a
//! report the harness branches on. Every call is also handed to a sink
//! so a session's verdicts can be reviewed later.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Error;
use crate::judge::Judge;
use crate::question::{Answer, Judgment, Questions, State, Usage};
use crate::rubric::{Action, Rubric};

/// A compact view of the agent's situation, shaped for the built-in
/// rubrics. A harness fills what it has; missing parts are omitted
/// from the state rather than sent as null.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// The user's request, as they wrote it (cap it).
    pub task: String,
    /// Standing instructions the agent must not violate, in plain words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    /// The tool call that just finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolCall>,
    /// Earlier tool calls this turn, oldest first, for repetition checks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent_tools: Vec<ToolCall>,
    /// The assistant's latest text, for turn-end checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_text: Option<String>,
    /// Anything else a custom rubric wants to reference.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default)]
    pub is_error: bool,
}

/// Output cap so a large tool result does not blow the judge's
/// context. Head and tail are kept; the middle is elided.
pub const OUTPUT_CAP: usize = 4000;

impl Observation {
    pub fn new(task: impl Into<String>) -> Self {
        Self {
            task: task.into(),
            ..Default::default()
        }
    }

    pub fn constraint(mut self, c: impl Into<String>) -> Self {
        self.constraints.push(c.into());
        self
    }

    pub fn tool(mut self, call: ToolCall) -> Self {
        self.tool = Some(call.capped());
        self
    }

    pub fn recent(mut self, call: ToolCall) -> Self {
        self.recent_tools.push(call.digest());
        self
    }

    pub fn assistant_text(mut self, text: impl Into<String>) -> Self {
        self.assistant_text = Some(cap(&text.into(), OUTPUT_CAP));
        self
    }

    pub fn extra(mut self, key: impl Into<String>, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }

    /// The judge's state for this observation, with credential-shaped
    /// strings replaced (see [`crate::redact`]). This is the only form
    /// that leaves the process or reaches a sink.
    pub fn to_state(&self) -> State {
        let mut state = serde_json::to_value(self).expect("observation serializes");
        crate::redact::value(&mut state);
        state
    }
}

impl ToolCall {
    pub fn new(name: impl Into<String>, input: Value) -> Self {
        Self {
            name: name.into(),
            input,
            output: None,
            is_error: false,
        }
    }

    pub fn output(mut self, output: impl Into<String>, is_error: bool) -> Self {
        self.output = Some(output.into());
        self.is_error = is_error;
        self
    }

    fn capped(mut self) -> Self {
        if let Some(o) = self.output.take() {
            self.output = Some(cap(&o, OUTPUT_CAP));
        }
        cap_strings(&mut self.input, OUTPUT_CAP);
        self
    }

    /// A short form for the recent-tools list: name, input, and output,
    /// each string cut to `DIGEST_CAP` characters.
    fn digest(mut self) -> Self {
        if let Some(o) = self.output.take() {
            self.output = Some(cap(&o, DIGEST_CAP));
        }
        cap_strings(&mut self.input, DIGEST_CAP);
        self
    }
}

/// Cap for each string in a recent-tools entry.
pub const DIGEST_CAP: usize = 200;

/// Cap every string inside a JSON value. Tool input can hold a whole
/// file (a write), which the judge does not need.
fn cap_strings(v: &mut Value, max: usize) {
    match v {
        Value::String(s) if s.chars().count() > max => *s = cap(s, max),
        Value::Array(items) => items.iter_mut().for_each(|x| cap_strings(x, max)),
        Value::Object(map) => map.values_mut().for_each(|x| cap_strings(x, max)),
        _ => {}
    }
}

fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max * 2 / 3).collect();
    let tail: String = s
        .chars()
        .rev()
        .take(max / 3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}\n[... elided ...]\n{tail}")
}

/// The most severe fired action, or `Proceed` when nothing fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Recommendation {
    Proceed,
    Warn,
    Escalate,
    Halt,
}

impl From<Action> for Recommendation {
    fn from(a: Action) -> Self {
        match a {
            Action::Note => Recommendation::Proceed,
            Action::Warn => Recommendation::Warn,
            Action::Escalate => Recommendation::Escalate,
            Action::Halt => Recommendation::Halt,
        }
    }
}

/// One answer, with the rubric's reading of it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub rubric: String,
    pub question: String,
    pub answer: Answer,
    /// `yes`, `no`, or `uncertain` for a noul; the option or level otherwise.
    pub label: String,
}

/// A rule that fired.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub rubric: String,
    pub question: String,
    pub action: Action,
    pub reason: String,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub recommendation: Recommendation,
    pub findings: Vec<Finding>,
    pub verdicts: Vec<Verdict>,
    pub model: String,
    pub usage: Usage,
    pub latency_ms: u64,
}

impl Report {
    pub fn verdict(&self, rubric: &str, question: &str) -> Option<&Verdict> {
        self.verdicts
            .iter()
            .find(|v| v.rubric == rubric && v.question == question)
    }

    /// P(yes) for a noul, by rubric and question id.
    pub fn noul(&self, rubric: &str, question: &str) -> Option<f64> {
        match self.verdict(rubric, question)?.answer {
            Answer::Noul { noul } => Some(noul),
            _ => None,
        }
    }

    pub fn halted(&self) -> bool {
        self.recommendation == Recommendation::Halt
    }
}

/// What gets handed to a sink after every call: the full exchange, so a
/// session can be replayed and thresholds re-tuned offline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub at_unix_ms: u128,
    /// Caller-supplied context such as a session id or hook event.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    pub judge: String,
    pub rubrics: Vec<String>,
    pub state: State,
    pub questions: Questions,
    pub report: Report,
}

pub trait Sink: Send + Sync {
    fn record(&self, record: &Record);
}

/// Appends one JSON object per line. Creates the parent directory if
/// needed and, on Unix, creates the file readable by the owner only,
/// because records hold tool output. A write failure goes to stderr and
/// never fails the supervise call.
pub struct JsonlSink {
    path: std::path::PathBuf,
}

impl JsonlSink {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn append(&self, record: &Record) -> std::io::Result<()> {
        use std::io::Write;
        if let Some(dir) = self.path.parent()
            && !dir.as_os_str().is_empty()
        {
            std::fs::create_dir_all(dir)?;
        }
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut line = serde_json::to_string(record)?;
        line.push('\n');
        opts.open(&self.path)?.write_all(line.as_bytes())
    }
}

impl Sink for JsonlSink {
    fn record(&self, record: &Record) {
        if let Err(e) = self.append(record) {
            eprintln!(
                "truthsayer: cannot write record to {}: {e}",
                self.path.display()
            );
        }
    }
}

pub struct Supervisor {
    judge: Arc<dyn Judge>,
    rubrics: Vec<Rubric>,
    sinks: Vec<Arc<dyn Sink>>,
    labels: BTreeMap<String, String>,
}

impl Supervisor {
    pub fn new(judge: Arc<dyn Judge>) -> Self {
        Self {
            judge,
            rubrics: Vec::new(),
            sinks: Vec::new(),
            labels: BTreeMap::new(),
        }
    }

    /// Attach a label to every record this supervisor writes.
    pub fn label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.labels.insert(key.into(), value.into());
        self
    }

    /// Add a rubric. Rubric names must be unique.
    pub fn rubric(mut self, rubric: Rubric) -> Self {
        self.rubrics.push(rubric);
        self
    }

    pub fn rubrics<I: IntoIterator<Item = Rubric>>(mut self, rubrics: I) -> Self {
        self.rubrics.extend(rubrics);
        self
    }

    pub fn sink(mut self, sink: Arc<dyn Sink>) -> Self {
        self.sinks.push(sink);
        self
    }

    pub fn rubric_names(&self) -> Vec<&str> {
        self.rubrics.iter().map(|r| r.name.as_str()).collect()
    }

    /// Ask every loaded rubric about `obs` in one call.
    pub async fn supervise(&self, obs: &Observation) -> Result<Report, Error> {
        let names: Vec<&str> = self.rubrics.iter().map(|r| r.name.as_str()).collect();
        self.supervise_with(obs, &names).await
    }

    /// Ask only the named rubrics. Unknown names are an error.
    pub async fn supervise_with(
        &self,
        obs: &Observation,
        rubric_names: &[&str],
    ) -> Result<Report, Error> {
        let mut chosen = Vec::new();
        for name in rubric_names {
            let r = self
                .rubrics
                .iter()
                .find(|r| &r.name == name)
                .ok_or_else(|| Error::Config(format!("rubric `{name}` is not loaded")))?;
            chosen.push(r);
        }
        let state = obs.to_state();
        let questions = fan_out(&chosen);
        let started = Instant::now();
        let judgment = self.judge.judge(&state, &questions).await?;
        let latency_ms = started.elapsed().as_millis() as u64;
        let report = apply(&chosen, &judgment, latency_ms)?;

        if !self.sinks.is_empty() {
            let record = Record {
                at_unix_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0),
                labels: self.labels.clone(),
                judge: self.judge.name().to_string(),
                rubrics: chosen.iter().map(|r| r.name.clone()).collect(),
                state,
                questions,
                report: report.clone(),
            };
            for sink in &self.sinks {
                sink.record(&record);
            }
        }
        Ok(report)
    }
}

/// Merge every rubric's questions into one request. Ids are namespaced
/// `rubric.question` so rubrics never collide; the id is not shown to
/// the model.
pub fn fan_out(rubrics: &[&Rubric]) -> Questions {
    let mut all = Questions::new();
    for r in rubrics {
        for (id, q) in &r.questions {
            all.insert(format!("{}.{}", r.name, id), q.clone());
        }
    }
    all
}

/// Turn a judgment into verdicts and findings by applying each rubric's
/// rules in code.
pub fn apply(rubrics: &[&Rubric], judgment: &Judgment, latency_ms: u64) -> Result<Report, Error> {
    let mut verdicts = Vec::new();
    let mut findings = Vec::new();
    let mut recommendation = Recommendation::Proceed;

    for r in rubrics {
        for id in r.questions.keys() {
            let key = format!("{}.{}", r.name, id);
            let answer = judgment
                .answers
                .get(&key)
                .ok_or_else(|| Error::MissingAnswer(key.clone()))?;
            let label = match answer {
                Answer::Noul { noul } if *noul > r.uncertain[0] && *noul < r.uncertain[1] => {
                    "uncertain".to_string()
                }
                other => other.label(),
            };
            verdicts.push(Verdict {
                rubric: r.name.clone(),
                question: id.clone(),
                answer: answer.clone(),
                label,
            });
        }
        for rule in &r.rules {
            let key = format!("{}.{}", r.name, rule.question);
            let answer = &judgment.answers[&key];
            if rule.when.holds(answer, r.uncertain) {
                let reason = if rule.reason.is_empty() {
                    format!(
                        "{} fired on `{}`",
                        serde_json::to_string(&rule.when).unwrap_or_default(),
                        rule.question
                    )
                } else {
                    rule.reason.clone()
                };
                findings.push(Finding {
                    rubric: r.name.clone(),
                    question: rule.question.clone(),
                    action: rule.then,
                    reason,
                    value: answer.value(),
                });
                recommendation = recommendation.max(rule.then.into());
            }
        }
    }
    findings.sort_by_key(|f| std::cmp::Reverse(f.action));
    Ok(Report {
        recommendation,
        findings,
        verdicts,
        model: judgment.model.clone(),
        usage: judgment.usage.clone(),
        latency_ms,
    })
}

/// A one-line summary a harness can print or log.
pub fn summarize(report: &Report) -> String {
    let mut s = format!("{:?}", report.recommendation).to_lowercase();
    for f in &report.findings {
        s.push_str(&format!(
            " | {:?} {}.{}={:.2}: {}",
            f.action, f.rubric, f.question, f.value, f.reason
        ));
    }
    s.push_str(&format!(
        " | {}ms ${:.6}",
        report.latency_ms, report.usage.cost_usd
    ));
    s
}
