# Harness integration

This page tells you where an agent harness calls the supervisor, what data it gives, and what it does with the report. The Claude Code hook in `crates/truthsayer-cli` is the worked example.

## The three points in the loop

Each agent loop has the same three points. A harness needs one hook at each point.

| Point | Rubrics | What the harness does with the report |
| --- | --- | --- |
| **Tool end.** A tool call is complete, and the model did not see the output yet. | `tool-result`, `progress`, and `edit` if the tool changed a file | `halt`: return an error result for the tool call and end the turn. `escalate`: ask the user for approval. `warn`: record the finding, or add it to the tool result. |
| **Before the next model call** | `model-tier` | Select a model for the tier. Use this rubric only if the harness selects between models. |
| **Turn end.** The model stopped with text. | `turn-end` | `warn` on a claim that is not verified: show it to the user, or give the model one more turn to verify the claim. |

All other decisions stay in the harness. Examples are exit codes, approvals, budgets, and the session file.

The supervisor does not see the prompt and does not change the prompt. Thus, a cached prompt prefix does not change.

## Build the observation

`Observation` is small on purpose. The harness adds the data that it has at that point:

```rust
let obs = Observation::new(&turn.user_text)                 // shorten long prompts
    .constraint("Do not modify anything under src/auth")    // from configuration or project rules
    .tool(ToolCall::new(name, input.clone()).output(output_text, is_error))
    .recent(earlier_call_1)                                  // earlier calls in this turn, oldest first
    .recent(earlier_call_2);
```

The supervisor shortens each string in a `recent_tools` entry to 200 characters. It shortens each string in `tool` to 4000 characters and keeps the start and the end. Include only the calls of the current turn. Twelve entries are sufficient for the repetition check.

Constraints are plain sentences. A harness can get them from a configuration key, a project file, or the user's message. The `edit` rubric checks the edit against the listed constraints only.

## Worked example: Claude Code

Claude Code has hook events at the same points. The `truthsayer hook` command connects them:

| Point | Claude Code event | Rubrics |
| --- | --- | --- |
| Before an edit | `PreToolUse` with the tool `Edit`, `Write`, `MultiEdit`, or `NotebookEdit` | `edit` |
| Tool end | `PostToolUse` and `PostToolUseFailure` | `tool-result`, `progress` |
| Turn end | `Stop` | `turn-end` |

The hook gets the data for the observation from two sources:

- **The hook input.** It contains the tool name, the tool input, the tool output or the error, and Claude's final message at a stop.
- **The session transcript.** The hook reads the transcript for the user's prompt and for the earlier tool calls in the turn. Claude Code writes the transcript asynchronously. Thus, the hook takes the current call from the hook input only.

If the transcript does not contain a prompt, the hook removes the questions that refer to `` `task` ``. Without the task, these questions give answers that are not useful.

The hook changes the report into Claude Code's response format:

| Recommendation | Before an edit | After a tool call | At a stop |
| --- | --- | --- | --- |
| `halt` | Deny the edit. Claude gets the reason. | Block, and give Claude the reason. | Give Claude the finding. |
| `escalate` | Ask the user to approve the edit. | Give Claude the finding, and show it to the user. | Give Claude the finding. |
| `warn` | Give Claude the finding. | Give Claude the finding. | Give Claude the finding, and Claude continues one time. |

The hook does these steps in `enforce` mode only. For the other modes and the configuration, see [Use truthsayer with Claude Code](claude-code.md).

## Rules for a harness

These rules apply to each harness:

- **Do not fail the tool call.** If the judge fails or does not reply before the timeout, continue as if no finding exists. Record the error.
- **Start with records only.** Run the supervisor without effect for some time. Read the records before you let a finding stop a turn.
- **Limit the continuations.** If a finding can make the model continue, permit only one continuation for each stop.
- **Treat project files as untrusted.** A file in the repository can add constraints or turn checks off. It must not change the backend, the endpoint, the model, or the record location.
- **Keep the state small.** The judge reads the whole state that it receives. List only the necessary paths in the `state` field of each rubric.
- **Redact before you send.** `Observation::to_state` redacts common secret formats. Also remove data that you know is secret.

## Cost and latency

A tool-end check costs approximately $0.00005. On an open connection, a request to the TypeSafe API took 140 to 250 ms in the author's tests. The Claude Code hook starts a new process and a new TLS connection for each event, so each check took 500 to 650 ms. A turn with 40 tool calls costs approximately 0.2 cents.

To hide the latency, run `supervise` at the same time as the next model call. Wait for the result only before the next tool call starts.

The Claude Code hook uses a different method in `log` mode. It runs the judge call in a background process, because in that mode the result does not change what Claude does.

## Other languages

The same three points exist in each agent loop. The roadmap includes TypeScript and Python SDKs. These SDKs read the same rubric files and use the same `Observation` and `Supervisor` types. Thus, a harness in a different language can use the same rubrics.
