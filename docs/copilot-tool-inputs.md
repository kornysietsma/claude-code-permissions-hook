# Copilot CLI hook payloads

What GitHub Copilot CLI sends to a `preToolUse` hook on stdin, and so what `match."<path>"` rules can address. Rule field paths are dotted paths into this JSON, e.g. `toolArgs.command` or `cwd`.

> **Unverified.** The top-level fields come from GitHub's [hooks reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-hooks-reference) (compiled 2026-10-03). The field names inside `toolArgs` are **best guesses** and have not been checked against a real Copilot CLI. They are confirmed or corrected from captured payloads in plan step 9.2; until then, treat every `toolArgs.*` name below as a guess.
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
| `toolName` | Matched by a rule's `tool`. Copilot's tool names are lowercase: `bash`, `view`, `create`, `edit`, `glob`, `grep`, `rg`, `task`, `web_fetch`, ... |
| `cwd` | Required by this hook; also what `{cwd}` expands to in `under`. |
| `toolArgs` | A parsed object of per-tool arguments. |
| `sessionId`, `timestamp` | Unix milliseconds. Available to rules. |

A payload without a string `toolName` and `cwd` is not treated as a Copilot payload: it passes through with a warning on stderr. In particular, Copilot also reads hooks from `.claude/settings.json`; if one of those fires, the hook receives Claude's snake_case payload and ignores it. Don't rely on `.claude/` hooks under Copilot.

## Per-tool `toolArgs` (unverified guesses)

| Tool | Fields assumed |
|------|----------------|
| `bash` | `command`, `description` |
| `view` | `path` |
| `create` | `path`, `file_text` |
| `edit` | `path`, `old_str`, `new_str` |
| `glob` | `pattern` |
| `task` | `description`, `prompt`, `agent_type` |

## Decision output

```json
{ "permissionDecision": "allow|deny|ask", "permissionDecisionReason": "..." }
```

No output means Copilot continues its normal permission flow. Copilot treats any non-zero exit code as a deny, so `tool-gate-hook run` always exits 0, and a reason is always sent with a deny.
