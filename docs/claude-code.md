# Use truthsayer with Claude Code

This page describes the truthsayer plugin for Claude Code. It tells you what each hook does, how to configure truthsayer, and which data truthsayer sends and keeps.

For installation, see the [README](../README.md#install).

## How the plugin works

The plugin contains a hook configuration and a small shell script. The script runs the `truthsayer` binary with the `hook` command. Claude Code sends each event to the binary as JSON on standard input. The binary replies on standard output.

| Event | Rubrics | Effect in `enforce` mode |
| --- | --- | --- |
| `SessionStart` | None | Shows setup problems to you in all modes. |
| `PreToolUse` for `Edit`, `Write`, `MultiEdit`, and `NotebookEdit` | `edit` | Halt: denies the edit and gives Claude the reason. Escalate: asks you to approve the edit. Warn: gives Claude the finding. |
| `PostToolUse` | `tool-result`, `progress` | Gives Claude the findings. Escalate: also shows the finding to you. |
| `PostToolUseFailure` | `tool-result`, `progress` | Gives Claude the findings. |
| `Stop` | `turn-end` | Gives Claude the finding, and Claude continues one more time. |

At `SessionStart`, truthsayer reports a configuration file that is not valid. If the mode is not `off`, it also reports a missing API key.

truthsayer does not check tools that only manage the session, for example `TodoWrite`, `AskUserQuestion`, and `ExitPlanMode`.

Each finding that truthsayer gives Claude starts with a statement that an automatic check made the finding and that the finding can be wrong. The finding also contains a recommended action. For a claim that is not verified, Claude must run a check or tell you that the claim is not verified.

### Failures

When a check fails, truthsayer prints nothing and exits with code 0. Claude Code then continues as if the hook is not installed. These conditions cause a failed check:

- The API key is not set.
- The judge does not reply before the timeout.
- The judge returns an error.
- The hook input is not valid JSON.

truthsayer writes the cause to standard error. In `log` mode, it writes the cause to `hook.log` in the record directory.

### Stop loops

At a stop, truthsayer asks Claude to continue only when the stop is not already the result of a stop hook. Claude Code sets `stop_hook_active` in that condition. Thus, each stop gets a maximum of one extra turn from truthsayer.

## Modes

| Mode | Judge call | Record | What you see | What Claude sees |
| --- | --- | --- | --- | --- |
| `off` | No | No | Nothing | Nothing |
| `log` | Yes, in the background | Yes | Nothing | Nothing |
| `advise` | Yes | Yes | Each finding | Nothing |
| `enforce` | Yes | Yes | Findings that need your approval | Each finding, with a recommended action |

The default mode is `log`. In `log` mode, the hook starts a background process for the judge call and exits immediately. Thus, `log` mode does not make tool calls slower.

In `advise` and `enforce` modes, each checked event waits for the judge. One judge call usually takes 300 to 1000 ms.

Only findings with the action `warn`, `escalate`, or `halt` have an effect. truthsayer writes findings with the action `note` to the record only.

## Configuration

truthsayer reads two configuration files. Both files are optional.

| File | Location | Who controls it |
| --- | --- | --- |
| User file | `$TRUTHSAYER_CONFIG`, or `$XDG_CONFIG_HOME/truthsayer/config.toml`, or `~/.config/truthsayer/config.toml` | You |
| Project file | `.claude/truthsayer.toml` in the project directory | The repository |

The `TRUTHSAYER_MODE` environment variable overrides the mode in both files.

### Keys in the user file

| Key | Default | Description |
| --- | --- | --- |
| `mode` | `"log"` | `off`, `log`, `advise`, or `enforce`. |
| `constraints` | `[]` | Rules that the agent must obey, in plain language. The `edit` rubric checks each edit against these rules. If you set no constraints, truthsayer does not ask the constraint question. |
| `skip` | `[]` | Names of rubrics that truthsayer does not run, for example `["progress"]`. |
| `record` | `~/.local/state/truthsayer/records.jsonl` | The record file. Set `false` to stop records. |
| `timeout_ms` | `8000` | The maximum time for one judge call, in milliseconds. The permitted range is 500 to 60000. |
| `api_key_env` | `"OPENROUTER_API_KEY"` | The name of the environment variable that contains the API key. |
| `model` | `"~typesafe/jev-latest"` | The decision model. |
| `endpoint` | OpenRouter's decisions endpoint | The URL that receives each judge call. |

If you set `XDG_STATE_HOME`, the default record file is `$XDG_STATE_HOME/truthsayer/records.jsonl`.

### Keys in the project file

A project file comes with the repository. Thus, the person who wrote the repository controls it, and you possibly do not. For this reason, a project file can make truthsayer do less, but it cannot make it do more:

| Key | Effect |
| --- | --- |
| `mode` | Lowers the mode. A project file cannot raise the mode. For example, it can change `enforce` to `log`, but it cannot change `log` to `enforce`. |
| `constraints` | Adds rules to the rules from the user file. |
| `skip` | Adds rubrics to the list of rubrics that truthsayer does not run. |

A project file cannot set `endpoint`, `model`, `api_key_env`, `record`, or `timeout_ms`. These keys control where your data goes. If a project file contains one of these keys, truthsayer ignores the complete project file. The next `SessionStart` hook and `truthsayer doctor` then show the problem.

Example project file:

```toml
constraints = [
  "Do not modify generated files under src/gen.",
  "Do not add new dependencies.",
]
skip = ["progress"]
```

### Examine the configuration

To show the configuration that truthsayer uses in the current directory, run:

```sh
truthsayer doctor
```

The command exits with code 1 if it finds a problem.

## Records

In all modes except `off`, truthsayer adds one JSON line to the record file for each judge call. Each line contains these fields:

| Field | Contents |
| --- | --- |
| `at_unix_ms` | The time of the call. |
| `labels` | The session ID, the hook event, the mode, and the tool name. |
| `judge` | The model that answered. |
| `rubrics` | The rubrics in the call. |
| `state` | The data that truthsayer sent to the judge, after redaction. |
| `questions` | The questions that truthsayer sent. |
| `report` | Each answer, the findings, the recommendation, the latency, and the cost. |

Use the records to make sure that the thresholds are correct for your work. For example, find each `unverified_claim` finding and compare it with what really happened in that session.

On Unix, truthsayer creates the record file and `hook.log` with permission `0600`. Only your user can read them.

## Data and privacy

> [!IMPORTANT]
> Each judge call sends data to the endpoint in your configuration. By default, this endpoint is OpenRouter, and OpenRouter sends the request to TypeSafe.

A judge call contains these items:

- Your prompt for the current turn, to a maximum of 2000 characters.
- Your constraints.
- The tool name and the tool input. truthsayer shortens each string in the input to 4000 characters.
- The tool output, to a maximum of 4000 characters. truthsayer keeps the start and the end of longer output.
- For each earlier tool call in the turn: the name, the input, and the output. truthsayer shortens each string to 200 characters. The maximum is 12 earlier calls.
- At a stop: Claude's final message, to a maximum of 4000 characters.

Before truthsayer sends a judge call or writes a record, it does these steps:

1. It replaces common secret formats with `[redacted]`: API keys, GitHub, Slack, and AWS tokens, JSON web tokens, private key blocks, passwords in URLs, and bearer tokens.
2. It replaces the value in assignments whose names contain `SECRET`, `TOKEN`, `PASSWORD`, `API_KEY`, or a similar word. For example, `DB_PASSWORD=abc123` becomes `DB_PASSWORD=[redacted]`.
3. It does not send the contents of secret files. These files include `.env`, `.env.*`, `*.pem`, `*.key`, SSH private keys, `.netrc`, and `.pgpass`.

> [!WARNING]
> Redaction finds common secret formats only. It does not find all secrets. If a project contains data that must not leave your computer, add `mode = "off"` to the project file for that project.

## Remove truthsayer

To stop the checks for a short time, set `TRUTHSAYER_MODE=off` in your environment.

To remove truthsayer, do these steps:

1. In Claude Code, run `/plugin uninstall truthsayer@truthsayer`.
2. Run `cargo uninstall truthsayer-cli`.
3. Optional: delete the directory `~/.local/state/truthsayer`.
