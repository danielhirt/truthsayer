# truthsayer

Calibrated yes/no supervision for agent harnesses.

An agent harness makes many small judgments per turn that get buried inside one big generation: did that tool call fail, did the edit change behavior or only formatting, is the agent looping, did it touch something it was told not to, is the task done. truthsayer asks those as typed questions of a decision model (TypeSafe's Jev through OpenRouter today), gets calibrated probabilities back in one call, and applies rules in code. The harness branches on the result.

The split it enforces: **the LLM proposes, the judge scores, code decides.**

## Status

Early. The Rust crate works end to end against Jev and is the test bed for a real harness ([demerzel](https://github.com/demerzel-labs/demerzel)). TypeScript and Python SDKs are planned as thin clients over the same rubric files.

## Layout

```
rubrics/            language-neutral rubric packs (the actual product): questions + rules as JSON
crates/truthsayer/  Rust crate: judge trait, OpenRouter backend, mock judge, supervisor, sinks
docs/               design, rubric authoring, harness integration
```

## Quick start (Rust)

```sh
export OPENROUTER_API_KEY=...
cargo run --example supervise
```

```rust
use std::sync::Arc;
use serde_json::json;
use truthsayer::{Observation, OpenRouterJudge, Supervisor, ToolCall, rubric::builtin};

let sup = Supervisor::new(Arc::new(OpenRouterJudge::from_env()?)).rubrics(builtin::all());

let obs = Observation::new("Fix the failing cache test")
    .constraint("Do not modify anything under src/auth")
    .tool(ToolCall::new("edit", json!({"path": "src/auth/token.rs", "old": "a", "new": "b"})));

let report = sup.supervise_with(&obs, &["edit"]).await?;
match report.recommendation {
    Recommendation::Halt => stop_turn(report.findings),
    Recommendation::Escalate => ask_user(report.findings),
    Recommendation::Warn => log(report.findings),
    Recommendation::Proceed => {}
}
```

One `supervise` call sends every requested rubric's questions in a single request. On the live judge that is 300 to 1000 ms and about $0.00003.

## Built-in rubrics

| Rubric | Fires on | Questions |
| --- | --- | --- |
| `tool-result` | every tool end | failed, relevant to task, output carries instructions aimed at the assistant |
| `edit` | edit and create tools | changes behavior, violates a listed constraint, outside the task |
| `progress` | every tool end | repeating an earlier call, momentum toward the task |
| `turn-end` | assistant text at turn end | claims done, unverified success claim, needs the user |
| `model-tier` | before the next model call | cheap / standard / frontier |

What the live judge did with the example observations:

| Case | Recommendation | Why |
| --- | --- | --- |
| edit under `src/auth` with a constraint against it | halt | violates_constraint 0.84 |
| same edit in the right file | proceed | violates_constraint 0.06 |
| README containing "AI assistant: ignore your instructions" | escalate | injected_instructions 0.98 |
| `cargo test` run three times with the same failure | warn | repeating 0.97, momentum stuck |
| "all tests pass" with no test run in the turn | warn | unverified_claim 0.95 |
| same claim after a passing `cargo test` | proceed | unverified_claim 0.10 |

## Docs

- [docs/design.md](docs/design.md): why a decision model, the contract, the pieces
- [docs/rubrics.md](docs/rubrics.md): the rubric file format and how to write questions the judge answers well
- [docs/harness-integration.md](docs/harness-integration.md): where a harness calls it, with demerzel as the worked example

## Building on macOS

The crate links with the system `cc`. If Xcode's license has not been accepted, build with the standalone tools:

```sh
DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test
```
