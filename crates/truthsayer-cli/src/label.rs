//! `truthsayer label`: ask a person for the true answer to one question
//! on recorded calls, and append each answer to the truth file.
//!
//! Records are offered across the whole probability range, not only
//! where a rule fired: recall needs the true yeses that the judge
//! missed. The judge's answer is hidden by default so it does not bias
//! the person.

use std::io::{BufRead, Write};
use std::path::Path;

use serde_json::Value;
use truthsayer::{Answer, Question, Rubric};

use crate::store::{Loaded, Truth, TruthMap, append_truth, printable, state_path};

/// Longest text shown for one state path.
const SHOW_CAP: usize = 1500;

pub struct Options {
    /// `rubric.question`.
    pub question: String,
    pub limit: usize,
    pub show_answer: bool,
    pub relabel: bool,
}

/// The records to offer, in order: bands of the judge's value visited
/// in turn, so each session labels a spread of low, uncertain, and high
/// answers. Within a band, the order follows the record id, which is
/// stable and unrelated to time.
pub fn candidates<'a>(
    records: &'a [Loaded],
    key: &str,
    truth: &TruthMap,
    relabel: bool,
) -> Vec<(&'a Loaded, &'a Answer)> {
    let (rubric, question) = key.split_once('.').unwrap_or((key, ""));
    let mut bands: [Vec<(&Loaded, &Answer)>; 3] = Default::default();
    for rec in records {
        if !relabel && truth.contains_key(&(rec.id.clone(), key.to_string())) {
            continue;
        }
        let Some(v) = rec
            .record
            .report
            .verdicts
            .iter()
            .find(|v| v.rubric == rubric && v.question == question)
        else {
            continue;
        };
        let band = match &v.answer {
            Answer::Noul { noul } if *noul < 0.3 => 0,
            Answer::Noul { noul } if *noul < 0.7 => 1,
            Answer::Noul { .. } => 2,
            other => other.confidence().map_or(1, |c| usize::from(c >= 0.5) * 2),
        };
        bands[band].push((rec, &v.answer));
    }
    for b in &mut bands {
        b.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    }
    let mut out = Vec::new();
    let mut iters: Vec<_> = bands.into_iter().map(Vec::into_iter).collect();
    loop {
        let before = out.len();
        for it in iters.iter_mut() {
            if let Some(x) = it.next() {
                out.push(x);
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// The valid answers for a question, as (key the person types, stored value).
fn choices(q: &Question) -> Vec<(String, String)> {
    match q {
        Question::Noul { .. } => vec![("y".into(), "yes".into()), ("n".into(), "no".into())],
        Question::Choice { criteria, .. } => criteria
            .keys()
            .enumerate()
            .map(|(i, k)| ((i + 1).to_string(), k.clone()))
            .collect(),
        Question::Score { criteria, .. } => (0..criteria.len())
            .map(|i| (i.to_string(), i.to_string()))
            .collect(),
    }
}

fn show_value(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    let s = printable(&s);
    if s.chars().count() > SHOW_CAP {
        let head: String = s.chars().take(SHOW_CAP).collect();
        format!("{head}\n[... cut for display ...]")
    } else {
        s
    }
}

fn describe(q: &Question, out: &mut impl Write) -> std::io::Result<()> {
    writeln!(out, "Question: {}", printable(q.instructions()))?;
    match q {
        Question::Noul {
            criteria: Some(c), ..
        } => {
            writeln!(out, "  yes: {}", printable(&c.yes))?;
            writeln!(out, "  no:  {}", printable(&c.no))?;
        }
        Question::Noul { criteria: None, .. } => {}
        Question::Choice { criteria, .. } => {
            for (i, (k, d)) in criteria.iter().enumerate() {
                writeln!(out, "  {}) {k}: {}", i + 1, printable(d))?;
            }
        }
        Question::Score { criteria, .. } => {
            for (i, d) in criteria.iter().enumerate() {
                writeln!(out, "  {i}) {}", printable(d))?;
            }
        }
    }
    Ok(())
}

/// Run the labeling loop. Returns the number of answers saved.
pub fn run(
    records: &[Loaded],
    rubrics: &[Rubric],
    truth: &TruthMap,
    truth_file: &Path,
    opts: &Options,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<usize, String> {
    let (rubric_name, qid) = opts
        .question
        .split_once('.')
        .ok_or("name the question as rubric.question, for example turn-end.unverified_claim")?;
    let rubric = rubrics
        .iter()
        .find(|r| r.name == rubric_name)
        .ok_or_else(|| format!("no rubric `{rubric_name}`"))?;
    let question = rubric
        .questions
        .get(qid)
        .ok_or_else(|| format!("rubric `{rubric_name}` has no question `{qid}`"))?;
    let valid = choices(question);
    let keys: Vec<&str> = valid.iter().map(|(k, _)| k.as_str()).collect();

    let todo = candidates(records, &opts.question, truth, opts.relabel);
    let n = todo.len().min(opts.limit);
    let io = |e: std::io::Error| e.to_string();
    writeln!(
        out,
        "{n} records to label for {} ({} available)",
        opts.question,
        todo.len()
    )
    .map_err(io)?;
    let mut saved = 0;
    for (i, (rec, answer)) in todo.into_iter().take(n).enumerate() {
        let r = &rec.record;
        writeln!(
            out,
            "\n==== [{}/{n}] record {}  {} {}",
            i + 1,
            rec.id,
            r.labels.get("event").map_or("-", String::as_str),
            r.labels.get("tool").map_or("", String::as_str),
        )
        .map_err(io)?;
        for path in &rubric.state {
            if let Some(v) = state_path(&r.state, path) {
                writeln!(out, "--- {path}\n{}", show_value(v)).map_err(io)?;
            }
        }
        describe(question, out).map_err(io)?;
        if opts.show_answer {
            writeln!(out, "Judge: {} ({:.2})", answer.label(), answer.value()).map_err(io)?;
        }
        let stored = loop {
            write!(out, "Answer [{}], s to skip, q to quit: ", keys.join("/")).map_err(io)?;
            out.flush().map_err(io)?;
            let mut line = String::new();
            if input.read_line(&mut line).map_err(io)? == 0 {
                return Ok(saved);
            }
            match line.trim() {
                "q" => return Ok(saved),
                "s" => break None,
                typed => {
                    if let Some((_, v)) = valid.iter().find(|(k, _)| k == typed) {
                        break Some(v.clone());
                    }
                    writeln!(out, "Type one of: {}, s, q", keys.join(", ")).map_err(io)?;
                }
            }
        };
        if let Some(truth) = stored {
            append_truth(
                truth_file,
                &Truth {
                    id: rec.id.clone(),
                    question: opts.question.clone(),
                    truth,
                    at_unix_ms: now_ms(),
                },
            )
            .map_err(|e| format!("{}: {e}", truth_file.display()))?;
            saved += 1;
        }
    }
    Ok(saved)
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::tests::record;
    use crate::store::load_truth;
    use truthsayer::rubric::builtin;

    fn tmp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "truthsayer-label-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d.join("truth.jsonl")
    }

    #[tokio::test]
    async fn offers_a_spread_of_values() {
        let mut recs = Vec::new();
        for p in [0.95, 0.9, 0.1, 0.05, 0.5] {
            recs.push(record("tool-result", &[("tool-result.failed", p)]).await);
        }
        let c = candidates(&recs, "tool-result.failed", &TruthMap::new(), false);
        let first: Vec<f64> = c.iter().take(3).map(|(_, a)| a.value()).collect();
        assert!(first.iter().any(|v| *v < 0.3), "{first:?}");
        assert!(first.iter().any(|v| (0.3..0.7).contains(v)), "{first:?}");
        assert!(first.iter().any(|v| *v >= 0.7), "{first:?}");
    }

    #[tokio::test]
    async fn saves_answers_skips_and_stops_on_quit() {
        let recs = vec![
            record("tool-result", &[("tool-result.failed", 0.9)]).await,
            record("tool-result", &[("tool-result.failed", 0.1)]).await,
            record("tool-result", &[("tool-result.failed", 0.5)]).await,
        ];
        let path = tmp();
        let opts = Options {
            question: "tool-result.failed".into(),
            limit: 10,
            show_answer: false,
            relabel: false,
        };
        let mut input = std::io::Cursor::new("maybe\ny\ns\nq\n");
        let mut out = Vec::new();
        let saved = run(
            &recs,
            &builtin::all(),
            &TruthMap::new(),
            &path,
            &opts,
            &mut input,
            &mut out,
        )
        .unwrap();
        assert_eq!(saved, 1);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Type one of: y, n, s, q"), "{text}");
        assert!(!text.contains("Judge:"), "answer hidden by default");
        let truth = load_truth(&path);
        assert_eq!(truth.len(), 1);
        assert_eq!(truth.values().next().unwrap(), "yes");

        // Labeled records are not offered again.
        let left = candidates(&recs, "tool-result.failed", &truth, false);
        assert_eq!(left.len(), 2);
    }

    #[test]
    fn rejects_unknown_questions() {
        let opts = Options {
            question: "edit.nope".into(),
            limit: 1,
            show_answer: false,
            relabel: false,
        };
        let err = run(
            &[],
            &builtin::all(),
            &TruthMap::new(),
            Path::new("/dev/null"),
            &opts,
            &mut std::io::Cursor::new(""),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(err.contains("no question `nope`"), "{err}");
    }
}
