//! Reads the session transcript Claude Code passes as `transcript_path`.
//!
//! The transcript is JSONL. The fields read here are `type`,
//! `isSidechain`, `isMeta`, and `message.content`, where content is a
//! string or a list of blocks (`text`, `tool_use`, `tool_result`). The
//! format is not a stable interface, so every read is defensive: an
//! entry that does not parse is skipped, and a missing file gives an
//! empty view. The hook still works without a transcript; it only
//! loses the task text and the earlier calls.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;
use truthsayer::ToolCall;

/// Only the tail of a long transcript is read. A turn that is longer
/// than this loses its oldest calls, which the rubrics do not need.
const TAIL_BYTES: u64 = 2 * 1024 * 1024;

/// How many earlier calls go into `recent_tools`.
pub const RECENT_LIMIT: usize = 12;

/// What the hook needs from the transcript: the current turn only.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Turn {
    /// The user's prompt that opened the current turn.
    pub prompt: Option<String>,
    /// Tool calls in this turn, oldest first, with output when present.
    pub calls: Vec<Call>,
    /// The text of the last assistant message in this turn.
    pub last_assistant_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub id: String,
    pub call: ToolCall,
}

impl Turn {
    /// Earlier calls for the observation, excluding `current_id`, most
    /// recent `RECENT_LIMIT` only.
    pub fn recent_excluding(&self, current_id: Option<&str>) -> Vec<ToolCall> {
        let calls: Vec<&Call> = self
            .calls
            .iter()
            .filter(|c| Some(c.id.as_str()) != current_id)
            .collect();
        let skip = calls.len().saturating_sub(RECENT_LIMIT);
        calls
            .into_iter()
            .skip(skip)
            .map(|c| c.call.clone())
            .collect()
    }
}

pub fn read(path: &Path) -> Turn {
    match read_tail(path) {
        Some(text) => parse(&text),
        None => Turn::default(),
    }
}

fn read_tail(path: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut reader = BufReader::new(f);
    if start > 0 {
        // Drop the partial first line.
        let mut discard = Vec::new();
        reader.read_until(b'\n', &mut discard).ok()?;
    }
    let mut text = String::new();
    reader.read_to_string(&mut text).ok()?;
    Some(text)
}

pub fn parse(text: &str) -> Turn {
    let mut turn = Turn::default();
    let mut index: HashMap<String, usize> = HashMap::new();

    for line in text.lines() {
        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
        let Some(content) = entry.pointer("/message/content") else {
            continue;
        };
        match kind {
            "user" => {
                if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                // Prompts that a person did not type, such as scheduled
                // wakeups, do not start a new task.
                let origin = entry.pointer("/origin/kind").and_then(Value::as_str);
                let human = origin.is_none_or(|k| k == "human");
                if let Some(prompt) = user_prompt(content).filter(|_| human) {
                    // A new prompt opens a new turn.
                    turn = Turn {
                        prompt: Some(prompt),
                        ..Default::default()
                    };
                    index.clear();
                    continue;
                }
                for block in blocks(content) {
                    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
                        continue;
                    };
                    if let Some(&i) = index.get(id) {
                        let is_error = block.get("is_error").and_then(Value::as_bool) == Some(true);
                        let output = block.get("content").map(flatten).unwrap_or_default();
                        let call = turn.calls[i].call.clone().output(output, is_error);
                        turn.calls[i].call = call;
                    }
                }
            }
            "assistant" => {
                let mut text_parts = Vec::new();
                for block in blocks(content) {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(t) = block.get("text").and_then(Value::as_str) {
                                text_parts.push(t.to_string());
                            }
                        }
                        Some("tool_use") => {
                            let id = block.get("id").and_then(Value::as_str).unwrap_or("");
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
                            let input = block.get("input").cloned().unwrap_or(Value::Null);
                            index.insert(id.to_string(), turn.calls.len());
                            turn.calls.push(Call {
                                id: id.to_string(),
                                call: ToolCall::new(name, input),
                            });
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    turn.last_assistant_text = Some(text_parts.join("\n"));
                }
            }
            _ => {}
        }
    }
    turn
}

/// The prompt text if this user entry is a typed prompt rather than a
/// tool result.
fn user_prompt(content: &Value) -> Option<String> {
    match content {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            if items
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            {
                return None;
            }
            let text: Vec<&str> = items
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            (!text.is_empty()).then(|| text.join("\n"))
        }
        _ => None,
    }
}

fn blocks(content: &Value) -> impl Iterator<Item = &Value> {
    content.as_array().into_iter().flatten()
}

/// Tool result content is a string or a list of text blocks.
pub fn flatten(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(v: Value) -> String {
        format!("{v}\n")
    }

    fn sample() -> String {
        [
            line(json!({"type":"user","message":{"role":"user","content":"old prompt"}})),
            line(json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"t0","name":"Bash","input":{"command":"ls"}}]}})),
            line(json!({"type":"user","message":{"content":"Fix the failing cache test"}})),
            line(json!({"type":"assistant","message":{"content":[
                {"type":"text","text":"Running the tests."},
                {"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}}]}})),
            line(json!({"type":"user","message":{"content":[
                {"type":"tool_result","tool_use_id":"t1","is_error":true,"content":[{"type":"text","text":"test cache::evict ... FAILED"}]}]}})),
            line(json!({"type":"user","isMeta":true,"message":{"content":"<system reminder>"}})),
            line(json!({"type":"assistant","isSidechain":true,"message":{"content":[{"type":"tool_use","id":"s1","name":"Read","input":{}}]}})),
            "not json\n".to_string(),
            line(json!({"type":"assistant","message":{"content":[
                {"type":"tool_use","id":"t2","name":"Edit","input":{"file_path":"src/cache.rs"}}]}})),
            line(json!({"type":"assistant","message":{"content":[{"type":"text","text":"All tests pass now."}]}})),
        ]
        .concat()
    }

    #[test]
    fn keeps_only_the_current_turn() {
        let turn = parse(&sample());
        assert_eq!(turn.prompt.as_deref(), Some("Fix the failing cache test"));
        let ids: Vec<&str> = turn.calls.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["t1", "t2"]);
        assert_eq!(
            turn.last_assistant_text.as_deref(),
            Some("All tests pass now.")
        );
    }

    #[test]
    fn pairs_results_with_calls() {
        let turn = parse(&sample());
        let t1 = &turn.calls[0].call;
        assert!(t1.is_error);
        assert_eq!(t1.output.as_deref(), Some("test cache::evict ... FAILED"));
        assert_eq!(turn.calls[1].call.output, None);
    }

    #[test]
    fn excludes_the_current_call_from_recent() {
        let turn = parse(&sample());
        let recent = turn.recent_excluding(Some("t2"));
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].name, "Bash");
    }

    #[test]
    fn missing_file_is_empty() {
        assert_eq!(
            read(Path::new("/nonexistent/transcript.jsonl")),
            Turn::default()
        );
    }
}
