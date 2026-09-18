# Design

## The problem

A coding agent's loop is: model call, tool calls, model call, until the model ends the turn. Inside that loop the harness needs answers to small questions all the time. Did the command fail. Is this edit the kind that changes behavior. Has the agent run the same thing three times. Did the file it just read contain text telling it to do something. Is "all tests pass" backed by a test run.

Today those answers come from one of three places, none good:

- **Heuristics in code.** Exit codes, regexes on output, path globs. Cheap and testable, but they only catch what someone thought to write down.
- **The generating model itself.** The agent is asked to notice its own mistakes. It is slow, expensive, and the model grading its own work is the thing we do not trust.
- **A second LLM call.** Ask a chat model "is this a loop, answer yes or no". Slower than the step it guards, costs as much, and the answer is prose you parse and cannot threshold.

## The bet

A decision model gives a fourth option. TypeSafe's Jev (a "System One" model) takes a state and a set of typed questions and returns one constrained answer per question with a probability distribution. It is not a chat model and does not generate. Measured in [jev-lab](https://github.com/danielhirt/jev-lab): about 300 ms and $0.00004 per call, per-question std around 0.01 across repeats, and calibrated where a chat model at temperature 0 was confidently wrong.

That shape fits the harness problem exactly. Every judgment above is a yes/no or a pick-one over a closed set. They can all go in one request. The answer is a number you threshold, with an honest "I don't know" band. And the judge is a different model from the one being judged.

## The contract

```
Observation  --to_state()-->  State (JSON)
Rubrics      --fan_out()-->   Questions (one map, ids namespaced rubric.question)
Judge.judge(State, Questions) -> Judgment (one Answer per question, usage)
apply(Rubrics, Judgment)      -> Report (verdicts, findings, recommendation)
```

- **Observation** is the harness's compact view of the situation: the task, standing constraints, the tool call that just ended, the earlier calls this turn, the assistant's latest text. Large outputs are capped head and tail. Anything else goes in `extra`.
- **Rubric** is a named set of questions plus rules. It is a JSON file. Questions reference state by backticked path (`` `tool.output` ``). Rules say what to do when an answer crosses a threshold. See [rubrics.md](rubrics.md).
- **Judge** is a trait: anything that answers questions about a state. `OpenRouterJudge` is the live one. `MockJudge` scripts answers for tests. A rubric never knows which judge is behind it, so the decision model can change without touching a rubric.
- **Supervisor** owns a judge, a set of rubrics, and zero or more sinks. `supervise` sends every rubric in one call; `supervise_with` sends a named subset. It applies the rules and returns a report.
- **Report** carries a `recommendation` (the most severe fired action: proceed, warn, escalate, halt), the `findings` that fired, and every `verdict` so the harness can read raw probabilities when it wants a different rule.
- **Sink** receives the full exchange after every call: state, questions, report, latency, cost. `JsonlSink` appends to a file. This is the raw material for re-tuning thresholds offline and for any future dashboard.

## What the rules do and do not do

Rules are the code side of "code decides", written as data so they port between SDKs. A rule is one condition on one question mapped to one action. The supervisor applies them mechanically. It does not reason, weight, or combine. If a harness wants a composite ("halt if violates_constraint > 0.7 and changes_behavior > 0.5"), it reads the verdicts and writes that line itself. Keeping rules this simple is deliberate: every threshold is visible, testable with the mock judge, and re-tunable from sink records.

## What the judge is bad at

From TypeSafe's own jaggedness notes, confirmed in jev-lab: it reads literally, it does not count or do arithmetic, it does not compare dates, it degrades with irrelevant state, and it does not treat state as hostile. The rubrics are written around that. The observation carries only what the questions need. Anything numeric (exit codes, line counts, elapsed time) stays in the harness. And `injected_instructions` exists because the judge reading a hostile tool output is the same exposure the agent has; the rubric asks about it directly instead of hoping.

## Why rubrics are files

Three reasons. A tuned threshold is the product's accumulated value and should not be locked in one language's source. The TypeScript and Python SDKs will read the same files, so a rubric proven in one harness moves to another unchanged. And a harness owner can edit a question without a rebuild.

## Roadmap

1. Wire into demerzel behind a config flag; run on real sessions; collect sink records.
2. Re-tune thresholds from records. Add rubrics for what real sessions show is missing.
3. TypeScript SDK (`@truthsayer/sdk`), reusing the jev-lab client. Python after.
4. A record viewer: per-session verdict timeline, drift across sessions, cost.
