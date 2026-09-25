# Design

This page tells you why truthsayer uses a decision model, and how its parts connect.

## The problem

A coding agent works in a loop. It calls the model, runs the tool calls that the model requests, and calls the model again. The loop stops when the model ends the turn.

In this loop, the harness must answer small questions all the time:

- Did the command fail?
- Does this edit change behavior?
- Did the agent run the same command three times?
- Did the file that the agent read contain instructions to the agent?
- Did a test run confirm the claim "all tests pass"?

Today, these answers are from one of three sources. Each source has a problem:

- **Heuristics in code.** Exit codes, regular expressions, and path patterns are cheap, and you can test them. But they find only the cases that a person wrote as a rule.
- **The generating model.** The agent can examine its own work for mistakes. This is slow and expensive, and a model that grades its own work is not reliable.
- **A second chat model.** A second model can answer "Is this a loop? Answer yes or no." This call is as slow and as expensive as the step that it guards. The answer is text, so you cannot compare it with a threshold.

## The decision model

A decision model is a fourth source. TypeSafe's Jev is a decision model. It receives a state and a set of typed questions. It returns one answer for each question, with a probability distribution. It is not a chat model, and it does not generate text.

The author measured Jev `jev-1.13` in September 2026:

- One call took approximately 300 ms and cost approximately $0.00004.
- For each question, the standard deviation across repeated calls was approximately 0.01.
- On questions where the correct answer was not clear, Jev gave probabilities near 0.5. A chat model at temperature 0 gave confident answers on the same questions, and some of those answers were wrong.

These properties agree with the problem:

- Each question above has a yes-or-no answer or one answer from a closed set.
- One request can contain all the questions.
- The answer is a number that code can compare with a threshold. A band around 0.5 gives an honest "not known" result.
- The judge is a different model from the agent that it judges.

## The contract

```text
Observation  --to_state()-->  State (JSON, secrets redacted)
State        --project()-->   one State for each rubric: only the paths in its `state` list
Rubrics      --fan_out()-->   Questions (one map for each projection, IDs in the form rubric.question)
Judge.judge(State, Questions) -> Judgment (one answer for each question, and usage; requests run at the same time)
apply(Rubrics, Judgment)      -> Report (verdicts, findings, recommendation)
```

These are the parts:

- **Observation.** The harness's short description of the situation. It contains the task, the constraints, and the tool call that ended. It also contains the earlier calls in the turn and the assistant's last text. truthsayer shortens large outputs and keeps the start and the end. Other data goes in `extra`.
- **Rubric.** A named set of questions and rules, in a JSON file. Questions refer to the state with a path in backticks, for example `` `tool.output` ``. Rules tell what to do when an answer crosses a threshold. For the format, see [Rubrics](rubrics.md).
- **Judge.** A trait for a component that answers questions about a state. `HttpJudge` calls the live model through the TypeSafe API or through OpenRouter. `MockJudge` returns scripted answers for tests. A rubric does not know which judge answers it. Thus, you can change the decision model without a change to a rubric.
- **Supervisor.** Contains a judge, a set of rubrics, and zero or more sinks. The `supervise` method asks the questions of all rubrics. The `supervise_with` method asks the questions of the named rubrics only. Each rubric receives only its declared state paths. Rubrics with the same paths share one request. The supervisor applies the rules and returns a report.
- **Report.** Contains a recommendation, the findings, and each verdict. The recommendation is the most severe action that a rule gave: proceed, warn, escalate, or halt. The verdicts contain the raw probabilities, so a harness can apply its own rules.
- **Sink.** Receives the full exchange after each call: the state, the questions, the report, the latency, and the cost. `JsonlSink` adds each exchange to a file as one line and can rotate the file by size. Use these records to adjust the thresholds.
- **Redaction.** `Observation::to_state` replaces common secret formats before the state goes to the judge or to a sink. The Claude Code hook also removes the contents of secret files.

## Why each rubric sees only its own paths

The judge reads the whole state, not only the path that a question names. In a live session, a file that contained a prompt injection went into `recent_tools`. After that, three clean tool outputs got `injected_instructions` values of 0.61 to 0.69. The same three states, with only the paths of the `tool-result` rubric, got 0.02 to 0.07.

Thus, the `state` list of a rubric controls what the judge receives. Rubrics that read different paths go in different requests. The requests run at the same time, so the latency does not increase. The cost increases a little because some state goes in more than one request. To send one request with the full state, call `Supervisor::shared_state`.

## What the rules do

Rules are the "code decides" part of the design. They are data, so all SDKs can use the same rules. A rule has one condition on one question and one action. The supervisor applies the rules in sequence. It does not reason, weigh, or combine them.

If a harness needs a combined rule, the harness reads the verdicts and applies the rule in its own code. An example of a combined rule is "halt if `violates_constraint` > 0.7 and `changes_behavior` > 0.5".

This limit is intentional. With simple rules, you can see each threshold, test it with `MockJudge`, and adjust it from the records.

## What the judge does badly

TypeSafe publishes a list of the model's weak points. The author's measurements confirmed them:

- It reads literally.
- It does not count, calculate, or compare dates.
- Its answers become worse when the state contains data that is not related to the question.
- It does not treat the state as hostile.

The rubrics avoid these weak points. The observation contains only the data that the questions need. Numbers such as exit codes, line counts, and durations stay in the harness.

The judge reads the same tool output as the agent. Thus, hostile text in the output can affect the judge too. The `injected_instructions` question asks about this risk directly.

## Why rubrics are files

There are three reasons:

1. A threshold that you adjust from real records is the valuable part of the product. Thus, it belongs in data, not in the source code of one language.
2. SDKs in other languages can read the same files. A rubric that works in one harness then works in a different harness without a change.
3. The owner of a harness can change a question without a new build.

## Roadmap

1. Collect records from real Claude Code sessions in `log` mode.
2. Label the records, and tune the thresholds with `report` and `replay`. For the process, see [Tune the thresholds](tuning.md). Add rubrics for the problems that real sessions show.
3. Stop the questions that a code heuristic answers as well as the judge.
4. Add a judge that uses log probabilities from a local model.
5. Publish TypeScript and Python SDKs that read the same rubric files.
