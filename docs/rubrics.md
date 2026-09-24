# Rubrics

A rubric is a JSON file. It contains a name, the questions to ask, and the rules that change answers into findings.

The Rust crate compiles the files in the `rubrics/` directory into `rubric::builtin`. These files are the source of truth for each SDK.

## Format

This example shows the three question types and two rules:

```json
{
  "name": "edit",
  "version": 1,
  "description": "What this rubric is for.",
  "state": ["task", "constraints", "tool.input"],
  "uncertain": [0.3, 0.7],
  "questions": {
    "violates_constraint": {
      "type": "noul",
      "instructions": "Does the edit described in `tool.input` do something that one of the `constraints` forbids?",
      "criteria": { "true": "...", "false": "..." }
    },
    "next_tier": {
      "type": "choice",
      "instructions": "...",
      "criteria": { "cheap": "...", "standard": "...", "frontier": "..." }
    },
    "momentum": {
      "type": "score",
      "instructions": "...",
      "criteria": ["Stuck: ...", "Some progress: ...", "Clear progress: ..."]
    }
  },
  "rules": [
    { "question": "violates_constraint", "when": { "at_least": 0.7 }, "then": "halt", "reason": "edit violates a standing constraint" },
    { "question": "violates_constraint", "when": { "uncertain": true }, "then": "escalate", "reason": "edit may violate a standing constraint" }
  ]
}
```

| Field | Description |
| --- | --- |
| `state` | The observation paths that the questions refer to. This field is for documentation. A harness can also use it to make sure that it supplies these paths. |
| `uncertain` | A noul that is strictly inside this band has the label `uncertain`, not `yes` or `no`. The default band is 0.3 to 0.7. |
| `questions` | Questions with an ID that you select. In requests and reports, the ID has the form `rubric.question`. |
| `rules` | The supervisor applies the rules in sequence. Each rule whose condition is true gives a finding. The recommendation of the report is the most severe action of all findings. |

### Question types

| Type | `criteria` | Answer |
| --- | --- | --- |
| `noul` | Optional. An object with a `true` description and a `false` description. | `noul`: the probability of yes. |
| `choice` | Required. A map from each option to its description. | `choice`, `probabilities` for each option, and `confidence`. |
| `score` | Required. A list of two or more level descriptions, from lowest to highest. | `score`: the expected level. Also `probabilities` for each level, and `confidence`. |

All instructions and criteria are plain strings. The OpenRouter adapter does not accept structured values.

### Rule conditions

Each rule has exactly one condition.

| Condition | True when |
| --- | --- |
| `at_least: x` | The main value is x or more. The main value is the probability of yes, the probability of the selected option, or the expected level. |
| `at_most: x` | The main value is x or less. |
| `uncertain: true` | A noul is inside the uncertain band of the rubric. |
| `is: "label"` | The label of the answer is equal to the given text. For a choice, the label is the option. For a score, the label is the level index as a string, for example `"0"`. |
| `confidence_below: x` | The confidence of a choice or a score is less than x. |

### Actions

| Action | Effect |
| --- | --- |
| `note` | Records a finding. The recommendation does not change. |
| `warn` | Continue, and show the finding. |
| `escalate` | Stop, and ask a person. |
| `halt` | Stop the turn. |

The recommendation of the report is the most severe action of all the rules that gave a finding.

## Write questions that the judge answers well

The judge reads literally and does not reason in more than one step. TypeSafe's documentation and the author's measurements give these guidelines:

1. **Name the state paths.** Write "Does `tool.output` show ...", not "Did the tool fail". The path tells the judge where to look.
2. **Put the policy in the state, and refer to it by path.** Ask "Does the edit break one of the `constraints`?" and put the constraints in the observation. Do not repeat the constraints in the question. Then a harness can change its constraints without a change to the rubric.
3. **Use the criteria for the difficult cases.** Put the examples that the judge can read incorrectly in the `true` and `false` descriptions. For example, tell the judge that "0 tests failed" is not a failure.
4. **Ask one thing in each question.** "Is it wrong and not related" is two questions. Write two questions, and combine the answers in a rule or in harness code.
5. **Keep numbers out of the questions.** Calculate exit codes, counts, durations, and dates in the harness. If the judge needs the result, give it as a word, for example "failed" or "third repeat".
6. **Make the instruction and the criteria agree.** If the `true` text of a noul describes a no, the judge answers badly.
7. **Do not expect the answers to agree with each other.** The probabilities of a question and its opposite do not always have a sum of 1. Ask for the thing that you want directly.
8. **Use a choice with a "none" option when "none of these" is a real result.** A choice compares the options with each other. A noul gives an absolute answer.

## Test a rubric

`MockJudge` returns scripted answers for each question ID. Thus, you can test the effect of a rule in a unit test:

```rust
let judge = MockJudge::new().noul("edit.violates_constraint", 0.5);
let report = Supervisor::new(Arc::new(judge)).rubric(builtin::edit()).supervise(&obs).await?;
assert_eq!(report.recommendation, Recommendation::Escalate);
```

To see how the live judge answers, run `cargo run --example supervise` and read the verdicts. When you change a question in a rubric, add a case to this example.

## Adjust thresholds from records

Attach a `JsonlSink`. For each call, the sink records the state, the questions, each answer, the findings, the latency, and the cost.

To adjust a threshold, do these steps:

1. Find the records that contain the question.
2. Compare the values with what really happened in those sessions.
3. Change the number in the rubric JSON file. You do not need to change code.
