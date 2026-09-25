# Tune the thresholds

This page tells you how to measure whether the checks are correct for your work, and how to change a threshold. Three commands do this work: `label`, `report`, and `replay`. None of them makes a judge call.

## Before you start

You need records from real sessions. Run the plugin in `log` mode for one or two weeks. For more information about modes and records, see [Use truthsayer with Claude Code](claude-code.md).

## The process

1. Label a sample of recorded answers for one question.
2. Measure the question with `report`.
3. Change a threshold in a copy of the rubric file.
4. Use `replay` to see which past recommendations the change affects.
5. Measure again on the holdout split. If the result is better, keep the change.

## Label answers

To give the true answer for one question, run:

```sh
truthsayer label --question turn-end.unverified_claim
```

For each record, the command shows the state that the rubric reads and the question. Type `y` or `n` for a yes-or-no question. For a choice or a score, type the number of the correct option. Type `s` to skip a record, or `q` to stop.

The command gives these features:

- **A spread of values.** It offers records with low, uncertain, and high judge values in turn. Thus, a short session gives data for both precision and recall.
- **No bias from the judge.** It does not show the judge's answer. To see it, add `--show-answer`.
- **Progress that you keep.** It saves each answer immediately to `truth.jsonl`, next to the record file. It does not offer a labeled record again. To change an answer, add `--relabel`.
- **Safe display.** It shows control characters in tool output as escape sequences. Thus, hostile output cannot change your terminal.

The default maximum is 25 records in one session. To change it, add `--limit`.

## Measure a question

To measure all labeled questions, run:

```sh
truthsayer report
```

For each yes-or-no question, the report shows:

| Section | Contents |
| --- | --- |
| Summary line | The number of labeled answers, the number of true yes answers, and the Brier score. A lower Brier score is better. |
| Calibration | For each band of judge values, the mean judge value and the share of true yes answers. In a calibrated judge, these two numbers are near each other. |
| Threshold sweep | Precision, recall, and F1 at thresholds from 0.1 to 0.9. An asterisk marks each threshold that a current rule uses. |
| Heuristic | Precision, recall, and F1 of a code-only rule on the same records, if one exists. |

The last section compares the judge with the heuristic for three questions: `turn-end.unverified_claim`, `progress.repeating`, and `tool-result.injected_instructions`. If the judge is not better than a regular expression for a question, the question does not need a judge call.

### Use the holdout split

The report assigns approximately 30% of the records to a holdout split. The assignment comes from the record ID, so it does not change between runs.

- Use `--split tune` to select thresholds.
- Use `--split holdout` to measure the result.

If you select a threshold and measure it on the same records, the result is too optimistic.

## Replay a change

To see the effect of a changed rule before you use it:

1. Copy a rubric file, for example `rubrics/turn-end.json`.
2. Change a threshold or a rule in the copy.
3. Run:

   ```sh
   truthsayer replay --rubric my-turn-end.json
   ```

The command applies the changed rubric to the recorded answers. It shows a table of recorded and new recommendations, and it lists the records that change.

`replay` warns you in two conditions:

- A question's wording changed after truthsayer recorded the answer. The recorded answer is for the old wording, so the result can be incorrect.
- A rule refers to a question that has no recorded answer. `replay` skips that rule.

To measure the changed thresholds, add the same `--rubric` option to `report`.

## Record files

truthsayer rotates the record file when it reaches 50 MB and keeps five old files. The three commands read the current file and all rotated files. To change these limits, set `record_max_mb` and `record_keep` in the user file.

By default, the commands read the record file from your configuration. To read a different file, add `--records PATH`.
