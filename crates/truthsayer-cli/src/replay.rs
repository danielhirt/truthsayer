//! `truthsayer replay`: apply the current rubrics, or edited rubric
//! files, to the answers already in the records. No judge calls are
//! made, so a threshold change can be checked against past sessions
//! for free.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use truthsayer::rubric::builtin;
use truthsayer::supervisor::apply;
use truthsayer::{Answers, Judgment, Recommendation, Report, Rubric};

use crate::store::Loaded;

/// The built-in rubrics, with any rubric in `files` replacing the
/// built-in rubric of the same name, or adding a new one.
pub fn load_rubrics(files: &[PathBuf]) -> Result<Vec<Rubric>, String> {
    let mut rubrics = builtin::all();
    for f in files {
        let r = Rubric::from_file(f).map_err(|e| format!("{}: {e}", f.display()))?;
        match rubrics.iter_mut().find(|x| x.name == r.name) {
            Some(slot) => *slot = r,
            None => rubrics.push(r),
        }
    }
    Ok(rubrics)
}

#[derive(Debug)]
pub struct Outcome {
    pub old: Recommendation,
    pub new: Recommendation,
    pub report: Report,
    /// Current questions with no recorded answer; their rules are skipped.
    pub unanswered: usize,
    /// Recorded questions whose instructions differ from the current
    /// rubric. Their answers are for the old wording.
    pub drifted: usize,
}

/// Re-apply `rubrics` to one record. `None` if none of the record's
/// rubrics is known.
pub fn replay_one(rec: &Loaded, rubrics: &[Rubric]) -> Option<Outcome> {
    let r = &rec.record;
    let answers: Answers = r
        .report
        .verdicts
        .iter()
        .map(|v| (format!("{}.{}", v.rubric, v.question), v.answer.clone()))
        .collect();

    let mut unanswered = 0;
    let mut drifted = 0;
    let mut chosen: Vec<Rubric> = Vec::new();
    for name in &r.rubrics {
        let Some(current) = rubrics.iter().find(|x| &x.name == name) else {
            continue;
        };
        let mut current = current.clone();
        current.questions.retain(|id, q| {
            let key = format!("{name}.{id}");
            if !answers.contains_key(&key) {
                unanswered += 1;
                return false;
            }
            if r.questions
                .get(&key)
                .is_some_and(|old| old.instructions() != q.instructions())
            {
                drifted += 1;
            }
            true
        });
        let kept = &current.questions;
        current
            .rules
            .retain(|rule| kept.contains_key(&rule.question));
        if !current.questions.is_empty() {
            chosen.push(current);
        }
    }
    if chosen.is_empty() {
        return None;
    }
    let refs: Vec<&Rubric> = chosen.iter().collect();
    let judgment = Judgment {
        model: r.report.model.clone(),
        answers,
        usage: r.report.usage.clone(),
    };
    let report = apply(&refs, &judgment, r.report.latency_ms).ok()?;
    Some(Outcome {
        old: r.report.recommendation,
        new: report.recommendation,
        report,
        unanswered,
        drifted,
    })
}

fn rec_name(r: Recommendation) -> &'static str {
    match r {
        Recommendation::Proceed => "proceed",
        Recommendation::Warn => "warn",
        Recommendation::Escalate => "escalate",
        Recommendation::Halt => "halt",
    }
}

const ORDER: [Recommendation; 4] = [
    Recommendation::Proceed,
    Recommendation::Warn,
    Recommendation::Escalate,
    Recommendation::Halt,
];

pub fn run(
    records: &[Loaded],
    rubrics: &[Rubric],
    show: usize,
    out: &mut impl Write,
) -> std::io::Result<()> {
    let mut matrix: BTreeMap<(Recommendation, Recommendation), usize> = BTreeMap::new();
    let mut changed = Vec::new();
    let (mut skipped, mut unanswered, mut drifted) = (0, 0, 0);
    for rec in records {
        let Some(o) = replay_one(rec, rubrics) else {
            skipped += 1;
            continue;
        };
        *matrix.entry((o.old, o.new)).or_default() += 1;
        unanswered += o.unanswered;
        drifted += o.drifted;
        if o.old != o.new {
            changed.push((rec, o));
        }
    }
    let total: usize = matrix.values().sum();
    writeln!(
        out,
        "replayed {total} records ({skipped} skipped: no known rubric)"
    )?;
    if total == 0 {
        return Ok(());
    }
    writeln!(out, "\nrecorded -> replayed")?;
    write!(out, "{:>10}", "")?;
    for n in ORDER {
        write!(out, "{:>10}", rec_name(n))?;
    }
    writeln!(out)?;
    for o in ORDER {
        write!(out, "{:>10}", rec_name(o))?;
        for n in ORDER {
            write!(out, "{:>10}", matrix.get(&(o, n)).copied().unwrap_or(0))?;
        }
        writeln!(out)?;
    }
    writeln!(out, "\n{} records change recommendation", changed.len())?;
    if drifted > 0 {
        writeln!(
            out,
            "warning: {drifted} recorded answers are for questions whose wording has changed since"
        )?;
    }
    if unanswered > 0 {
        writeln!(
            out,
            "note: {unanswered} current questions have no recorded answer; their rules were skipped"
        )?;
    }
    for (rec, o) in changed.iter().take(show) {
        let labels = &rec.record.labels;
        writeln!(
            out,
            "\n{}  {} {}  {} -> {}",
            rec.id,
            labels.get("event").map_or("-", String::as_str),
            labels.get("tool").map_or("", String::as_str),
            rec_name(o.old),
            rec_name(o.new),
        )?;
        for f in &o.report.findings {
            writeln!(
                out,
                "    {:?} {}.{} = {:.2}: {}",
                f.action, f.rubric, f.question, f.value, f.reason
            )?;
        }
    }
    if changed.len() > show {
        writeln!(
            out,
            "\n({} more; use --show to list them)",
            changed.len() - show
        )?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;
    use truthsayer::{MockJudge, Observation, Record, Sink, Supervisor, ToolCall};

    struct Keep(std::sync::Mutex<Vec<Record>>);
    impl Sink for Keep {
        fn record(&self, r: &Record) {
            self.0.lock().unwrap().push(r.clone());
        }
    }

    /// Build a record the way the hook does, with scripted answers.
    pub(crate) async fn record(rubric: &str, answers: &[(&str, f64)]) -> Loaded {
        let mut judge = MockJudge::new();
        for (id, p) in answers {
            judge = judge.noul(*id, *p);
        }
        let keep = Arc::new(Keep(Default::default()));
        let sup = Supervisor::new(Arc::new(judge))
            .rubrics(builtin::all())
            .sink(keep.clone())
            .label("event", "PostToolUse");
        let obs = Observation::new("Fix the test")
            .constraint("Do not touch src/auth")
            .tool(ToolCall::new("Read", serde_json::json!({"file_path": "a"})).output("x", false));
        sup.supervise_with(&obs, &[rubric]).await.unwrap();
        let record = keep.0.lock().unwrap().pop().unwrap();
        let line = serde_json::to_string(&record).unwrap();
        Loaded {
            id: truthsayer::records::record_id(&line),
            record,
        }
    }

    #[tokio::test]
    async fn unchanged_rubrics_reproduce_the_recording() {
        let rec = record("edit", &[("edit.violates_constraint", 0.8)]).await;
        let o = replay_one(&rec, &builtin::all()).unwrap();
        assert_eq!(o.old, Recommendation::Halt);
        assert_eq!(o.new, Recommendation::Halt);
        assert_eq!((o.unanswered, o.drifted), (0, 0));
    }

    #[tokio::test]
    async fn a_raised_threshold_changes_the_outcome() {
        let rec = record("edit", &[("edit.violates_constraint", 0.8)]).await;
        let mut edit = builtin::edit();
        edit.rules[0].when.at_least = Some(0.9);
        // 0.8 now sits below the halt threshold and above the band.
        let o = replay_one(&rec, &[edit]).unwrap();
        assert_eq!(o.new, Recommendation::Proceed);

        let mut buf = Vec::new();
        let rubrics = {
            let mut e = builtin::edit();
            e.rules[0].when.at_least = Some(0.9);
            vec![e]
        };
        run(&[rec], &rubrics, 5, &mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("1 records change recommendation"), "{text}");
        assert!(text.contains("halt -> proceed"), "{text}");
    }

    #[tokio::test]
    async fn reworded_questions_are_reported_as_drift() {
        let rec = record("edit", &[]).await;
        let mut edit = builtin::edit();
        if let Some(truthsayer::Question::Noul { instructions, .. }) =
            edit.questions.get_mut("outside_task")
        {
            instructions.push_str(" Reworded.");
        }
        let o = replay_one(&rec, &[edit]).unwrap();
        assert_eq!(o.drifted, 1);
    }
}
