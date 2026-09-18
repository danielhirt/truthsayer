# Harness integration

Where a harness calls the supervisor, what it passes, and what it does with the answer. demerzel is the worked example because it is the first host.

## The three seams

Every agent loop has the same three moments. A harness needs one hook at each.

| Moment | Rubrics | What the harness does with the report |
| --- | --- | --- |
| **Tool end**: a tool call finished, its output is in hand, the model has not seen it yet | `tool-result`, `progress`, and `edit` when the tool was an edit or create | `halt`: record an error result for the tool call and end the turn. `escalate`: pause for approval. `warn`: log or annotate the tool result. |
| **Before the next model call** | `model-tier` | Map the tier to a model id. Optional; only if the harness routes between models. |
| **Turn end**: the model stopped with text | `turn-end` | `warn` on an unverified claim: show it to the user, or feed one more user-less turn ("verify that claim") if the harness supports it. |

Everything else stays in the harness: exit codes, approvals, budgets, the session file. The supervisor never sees the prompt and never enters it, so a cached prompt prefix stays byte-stable.

## Building the observation

`Observation` is deliberately small. The harness fills what it has at that moment.

```rust
let obs = Observation::new(&turn.user_text)          // capped by the harness
    .constraint("Do not modify anything under src/auth")   // from config or the system prompt's rules
    .tool(ToolCall::new(name, input.clone()).output(output_text, is_error))
    .recent(earlier_call_1)                           // this turn's earlier calls, oldest first
    .recent(earlier_call_2);
```

`recent_tools` entries are digested (output cut to 200 chars). `tool.output` is capped at 4000 chars, head and tail. Keep the list to this turn; a dozen entries is plenty for repetition checks.

Constraints are plain sentences. They come from wherever the harness keeps rules: a config key, a project file, the user's message. The `edit` rubric judges only against what is listed.

## demerzel

demerzel's engine (`crates/demerzel-core/src/engine.rs`) already emits the events the seams need and already owns the approval policy, so the supervisor slots in as one more thing the engine consults before continuing.

**Where.** After the engine records a tool result and before the next model call: the point where `EngineEvent::ToolEnd { name, is_error, output, .. }` is emitted. The engine has the tool name, the input (`ToolRun`), the output, and the turn's history, which is everything `Observation` wants.

**Config.** A `supervisor` table in demerzel's config, off by default:

```toml
[supervisor]
enabled = true
rubrics = ["tool-result", "progress", "edit", "turn-end"]
constraints = ["Do not modify anything under src/auth"]
record = "~/.demerzel/supervisor.jsonl"
on_halt = "stop"        # stop | approve | log
```

**What to do with the report.** The engine already has `ApprovalDecision`. Map `Halt` to a denied dispatch with the finding's reason as the tool's error result, `Escalate` to an approval request in `ApprovalMode`'s interactive path, `Warn` to a line in the session record and a TUI card. The recommendation never changes the prompt; the tool result the model sees is the harness's usual one, plus an error string on halt.

**What to record.** Attach `JsonlSink` to the configured path. Every call lands with state, answers, findings, latency, and cost. The first real sessions are what the thresholds get tuned from, so run with `on_halt = "log"` first and read the records before letting it stop anything.

**Cost.** One call per tool end at about $0.00003 and 300 to 1000 ms. For a 40-tool turn that is a cent and under a minute of added wall time, most of it overlappable with rendering. If latency matters, run `supervise` concurrently with the next model call and only block on the result before dispatching the next tool.

**Dependency.** Add `truthsayer` as a path or git dependency in `crates/demerzel/Cargo.toml`. The crate pulls `reqwest` with rustls, which demerzel already uses, so no new TLS stack.

## Other harnesses

The same three seams exist in every loop. The TypeScript SDK will expose the same `Observation`, `Supervisor`, and rubric files, so a harness in another language integrates the same way with the same JSON.
