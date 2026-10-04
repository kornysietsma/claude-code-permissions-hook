# Copilot CLI hook payloads

What GitHub Copilot CLI sends to a `preToolUse` hook on stdin, and so what `match."<path>"` rules can address. Rule field paths are dotted paths into this JSON, e.g. `toolArgs.command` or `cwd`.

> **Checked against a real Copilot CLI on 2026-10-04** (macOS, one session with a GPT model; the CLI version was not recorded). The top-level fields come from GitHub's [hooks reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-hooks-reference); the per-tool `toolArgs` below are what that session sent. Copilot picks its tool set per model, so another model may use other tools (see the note under `apply_patch`).
>
> **The audit log is the authoritative source.** Set `level = "all"` and `max_value_len = 0` in `[audit]` to record exactly what your Copilot CLI version sends (see [tests/README.md](../tests/README.md)).

## Top-level fields

This hook handles Copilot's native camelCase `preToolUse` format:

```json
{
  "sessionId": "sess-1",
  "timestamp": 1760000000000,
  "cwd": "/Users/someone/project",
  "toolName": "bash",
  "toolArgs": { "command": "cargo test" }
}
```

| Field | Notes |
|-------|-------|
| `toolName` | Matched by a rule's `tool`. Copilot's tool names are lowercase. Seen: `bash`, `view`, `apply_patch`, `rg`, `glob`, `task`. GitHub's docs also list `create`, `edit`, `grep`, `powershell`, `web_fetch`, `web_search`, `ask_user` and `update_todo`. |
| `cwd` | Required by this hook; also what `{cwd}` expands to in `under`. |
| `toolArgs` | Per-tool arguments: **usually an object, but a string for `apply_patch`**. |
| `sessionId`, `timestamp` | `timestamp` is Unix milliseconds. Available to rules. |

A payload without a string `toolName` and `cwd` is not treated as a Copilot payload: it passes through with a warning on stderr. That is what a `--agent copilot` hook gets when it is registered in `.claude/settings.json`, which Copilot also reads (see below).

## Per-tool `toolArgs`

As captured on 2026-10-04.

| Tool | `toolArgs` | Example |
|------|------------|---------|
| `bash` | object: `command`, `description` | `{"command":"ls -la","description":"List files in the repository root"}` |
| `view` | object: `path` (absolute) | `{"path":"/Users/someone/project/README.md"}` |
| `glob` | object: `pattern`, `paths` (a directory, as a string) | `{"paths":"/Users/someone/project","pattern":"*.md"}` |
| `rg` | object: `pattern`, `paths` (a string), `output_mode`, `head_limit`, `-n` | `{"-n":true,"head_limit":100,"output_mode":"content","paths":"/Users/someone/project","pattern":"hello"}` |
| `task` | object: `agent_type`, `description`, `mode`, `name`, `prompt` | `{"agent_type":"task","description":"List repository files","mode":"sync","name":"repo-file-list","prompt":"..."}` |
| `apply_patch` | **a string**: the patch text | `"*** Begin Patch\n*** Update File: notes.txt\n@@\n-hello\n+goodbye\n*** End Patch\n"` |

Notes:

- **File changes arrive as `apply_patch`** with this model. Creating a file is `*** Add File: <path>` and changing one is `*** Update File: <path>`; the patch can also contain `*** Delete File:` and, in the patch format, `*** Move to:`. There is no path field and no `under` check: gate it with a regex on the whole string, for example `match."toolArgs" = { regex = '(?m)^\*\*\* (Add|Update|Delete) File: .*\.(env|secret)$' }`. Paths in a patch can be relative to `cwd`. When asked to "use the create tool" the model said it had none and used `apply_patch`, so `create` and `edit` are untested; they may still appear with other models.
- **`grep` arrives as `rg`**, with the search directory in `paths`.
- **`glob` and `rg` use `paths` (plural, a string), not `path`.** Only a single directory was seen; a list of paths has not been observed.
- **The task tool's name field is `task`** and its arguments include `prompt`, so a rule can match what a sub-agent is asked to do.
- The model sometimes declined to call a tool at all (for example refusing to view a file called `tgh-secret.txt`), so a deny rule is a backstop, not the only line of defence.

## Decision output

```json
{ "permissionDecision": "allow|deny|ask", "permissionDecisionReason": "..." }
```

No output means Copilot continues its normal permission flow. Copilot treats any non-zero exit code as a deny, so `tool-gate-hook run` always exits 0, and a reason is always sent with a deny.

What was observed:

| Decision | What the user sees |
|----------|--------------------|
| `deny` | The call is blocked and the UI shows `Denied by preToolUse hook: <reason>`; the model is told the same and relays it |
| `ask` | A prompt: `tool-gate-hook: ask by rule #4 (view of tgh-ask files)`, then the tool and its arguments, then "Do you want to allow this tool call?". This also works for tools that normally don't prompt (`glob`, `view`) |
| config error | An `ask` prompt showing the whole message, including the regex error and the rule number. After fixing the config the prompts stop |
| `allow` | The call runs. The test used `echo`, which Copilot seems to run without a prompt anyway, so "allow suppresses a prompt" is not yet demonstrated |

## Hooks and where they run

- **Every registered hook runs for every call**: the user-level hook, the repo-level hook and a `.claude/settings.json` hook each fired once per tool call.
- **Repo-level hooks run in the repository root**, so a relative `--config` such as `.github/hooks/tool-gate-hook.toml` works there (16 of 16 calls).
- **Copilot reads `.claude/settings.json` hooks and sends them a Claude-format payload.** The fields are `cwd`, `hook_event_name` (`PreToolUse`), `session_id`, `timestamp` (an ISO-8601 string, unlike the native hook), `tool_name` and `tool_input`. There is no `tool_use_id`, `permission_mode` or `transcript_path`. The tool names are mapped to Claude's: `bash` is `Bash`, `view` is `Read`, `apply_patch` is `Edit`, `glob` is `Glob`, `rg` is `Grep` and `task` is `Agent`. The tool input is the same object or string as the native `toolArgs`, so `Read` uses `tool_input.path` (not Claude's `file_path`) and `Edit` has a plain string for `tool_input`.
  - A `--agent claude` hook therefore works under Copilot, but a Claude config that gates `Read` by `tool_input.file_path` does not gate Copilot's reads. A `--agent copilot` hook registered there passes through with a warning.
  - Not tested: whether Copilot honours Claude-format decision output from such a hook, and whether it also reads the user-level `~/.claude/settings.json`.
