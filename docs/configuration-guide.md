# Configuration guide

The full reference for a `tool-gate-hook` config file. For a quick start see the [README](../README.md); for ready-to-copy configs see [`examples/`](../examples/).

A config is a TOML file with three parts: an optional `[audit]` table, an optional `[patterns]` table of named regexes, and any number of `[[rule]]` entries. Every config is for **one agent** (`--agent claude` or `--agent copilot`); write one file per agent.

Check a config with:

```bash
tool-gate-hook validate --agent claude --config path/to/claude.toml
```

`validate` exits non-zero on any error and prints a summary (rule counts per decision, pattern names, audit settings). It also **warns** (exit 0) when a rule's field path doesn't start with a payload key known for that agent, which catches rules copied between agents.

## Writing regexes in TOML

Write regexes as **single-quoted literal strings**, which need no backslash doubling:

```toml
regex = '^cargo (build|test)\b'     # good
regex = "^cargo (build|test)\\b"    # same thing, harder to read
```

## `[audit]`

```toml
[audit]
file = "/tmp/tool-gate-hook-claude.jsonl"
level = "matched"        # off | matched | all
max_value_len = 1024     # 0 = never truncate
```

Optional. Leaving it out disables auditing. When present, `file` is required (its directory must already exist), `level` defaults to `matched` and `max_value_len` to `1024`.

| `level` | Records |
|---------|---------|
| `off` | nothing |
| `matched` | calls where at least one rule matched, plus all error records |
| `all` | every call, including passthroughs |

See [Audit log](#audit-log) for the record format.

## `[patterns]`

Named regexes, reusable in `regex` and `not_regex`:

```toml
[patterns]
shell_chain = ';|\||`|&&|&[^0-9]|&$|\$\('
parent_dir  = '\.\.'
```

Patterns belong to the config file that defines them. Every pattern is compiled when the config loads, so a broken one is an error even if no rule uses it. [`examples/claude.toml`](../examples/claude.toml) ships commented `shell_chain` and `parent_dir` definitions to copy.

## `[[rule]]`

```toml
[[rule]]
decision = "allow"                       # required: allow | deny | ask
tool = "Bash"                            # optional: regex on the tool name
description = "npx mermaid-cli mmdc"     # optional: shown in logs and default reasons
reason = "..."                           # optional: what the agent is told on deny/ask
match."tool_input.command" = { regex = '^npx ', not_regex = "@shell_chain" }
```

| Key | Meaning |
|-----|---------|
| `decision` | Required. `allow`, `deny` or `ask`. |
| `tool` | A regex, **anchored** as `^(?:…)$` against the tool name, so `Read` doesn't match `ReadFile`. Use `Read\|Write` for either. Omit it to match any tool. Claude's names are capitalised (`Bash`), Copilot's lowercase (`bash`); matching is case-sensitive. |
| `description` | Free text used in audit records, validate warnings and default reasons. |
| `reason` | Returned to the agent as the decision reason. Default: `tool-gate-hook: <decision> by rule #<index>` plus ` (<description>)` when there is one. A reason is always sent for `deny` and `ask`. |
| `match` | A table keyed by dotted field path. All entries must pass (**AND**); for OR, write separate rules. A rule with no `match` entries matches on `tool` alone. |

Unknown keys anywhere in a rule are errors, so a typo like `comand` can't silently disable a rule.

### Field paths

Each `match` key is a dotted path into the **raw payload** the agent sends, so any field can be matched: `tool_input.command`, `toolArgs.path`, `cwd`, `permission_mode`, `agent_type`, and so on. See [Claude payloads](./claude-tool-inputs.md) and [Copilot payloads](./copilot-tool-inputs.md).

- A path walks objects by key. A **missing** path makes the matcher fail (the rule doesn't match), except for `exists = false`.
- Strings are matched as they are. Numbers and booleans are matched against their JSON text (`120000`, `true`). `null`, arrays and objects only work with `exists`.
- Quote the path in TOML: `match."tool_input.command" = { … }`.

## Matchers

Each field takes a table of matchers. **All matchers given must pass.**

| Matcher | Value | Passes when |
|---------|-------|-------------|
| `regex` | string or list | The value matches (an unanchored search: write `^` and `$` yourself). With a list, **any** item matching is enough. |
| `not_regex` | string or list | **No** item matches. This is the safety net: an excluded call makes the rule *not match*; it does not become a deny. |
| `equals` | string | The value is exactly this string. |
| `glob` | string | The value matches this glob, e.g. `**/*.rs`. Path-style: `*` does not cross `/`, `**` does. For simple path checks; use `regex` for commands. |
| `under` | list of strings | The value is a path inside any listed directory (see [`under`](#under)). |
| `exists` | bool | The field is present (`true`) or absent (`false`). The only matcher that can pass on a missing field. |

An empty matcher table (`match."x" = {}`) or an empty list (`regex = []`) is a config error.

```toml
# Either extension, but never a path that climbs upwards
match."tool_input.file_path" = { regex = ['\.md$', '\.txt$'], not_regex = "@parent_dir" }

# Only subagent calls (agent_id is absent for the main agent)
match."agent_id" = { exists = true }
```

### Pattern references and `@`

A list item (or string) starting with `@` is always a **pattern reference** to `[patterns]`. An unknown name is a config error. A regex that genuinely starts with a literal `@` must be written `\@` or `[@]`:

```toml
regex = '\@mention'     # a literal @
regex = '[@]mention'    # also fine
regex = '@mention'      # error: unknown pattern @mention
```

### `under`

`under` checks that a path lies inside one of the listed directories:

```toml
match."tool_input.file_path" = { under = ["{cwd}", "~/notes", "/tmp/mermaid"] }
```

- In the listed directories, `~` (alone or before `/`) is the home directory and `{cwd}` is the payload's `cwd`.
- A relative value is resolved against the payload's `cwd`.
- The path is **resolved the way the OS would**: the longest existing prefix is canonicalised (following symlinks, applying `..`), and only the not-yet-existing remainder is cleaned up textually, so a `Write` to a new file works. A symlink inside an allowed directory that points outside it does not escape the check, and `/tmp/mermaid/../secret` is not under `/tmp/mermaid`.
- Containment is by path component: `/tmp/mermaid2` is not under `/tmp/mermaid`.
- The listed directories are canonicalised the same way (so `/tmp` and macOS's `/private/tmp` agree).

Prefer `under` to a regex for path checks; it doesn't need a `parent_dir` safety net.

## How decisions are made

1. **Every** rule is evaluated, in file order, and all matches are collected.
2. The final decision is tiered, **deny > ask > allow**, regardless of file order.
3. The deciding rule is the first matching rule (in file order) with the winning decision, and its reason is used.
4. No rule matching means **passthrough**: nothing is printed and the agent's normal permission flow applies.

So a deny anywhere in the file beats any allow, and an `ask` forces a prompt even when an allow rule also matches. Use that to carve exceptions out of broad allows.

Shell chaining (`a && b`) is deliberately not parsed. Instead, give allow rules a `not_regex = "@shell_chain"` so a chained command makes the rule not match, and the call falls through to the user.

## Errors and failure behaviour

`tool-gate-hook run` **always exits 0**: Copilot treats any non-zero exit as a deny, and Claude treats exit 2 as a block. (A missing or unknown `--agent` is a command-line error and does exit 2; it shows up as soon as you edit the hook registration.)

| Situation | Behaviour |
|-----------|-----------|
| Config can't be loaded (missing file, bad TOML, invalid regex or glob, unknown `@pattern`, unknown key) | Output **`ask`** with the reason `tool-gate-hook config error (<path>): <details>`, and the same on stderr. Loud, but never locks you out. |
| stdin isn't valid JSON, or isn't from the `--agent` you configured | Passthrough, a warning on stderr, and an error record in the audit log if the config loads. Checked before the config, so a payload meant for another agent passes quietly even when the config is broken. |
| Audit file can't be written | A warning on stderr; the decision is unaffected. |

Diagnostics go to stderr; set `RUST_LOG=debug` for more.

## Audit log

One JSON object per line, appended under a file lock, so two hooks (for example a user-level and a project-level registration) can share one file; the `config` field tells their records apart.

```json
{
  "ts": "2026-10-03T17:42:01.123+10:00",
  "agent": "claude",
  "config": "/Users/someone/.config/tool-gate-hook/claude.toml",
  "decision": "allow",
  "decided_by": { "index": 3, "description": "rm mermaid test files" },
  "matches": [ { "index": 3, "decision": "allow", "description": "rm mermaid test files" } ],
  "payload": { "...": "the raw stdin JSON" },
  "duration_us": 412
}
```

- `decision` is `allow`, `ask`, `deny` or `passthrough`; `decided_by` is left out on passthrough. Rule `index` is the 1-based position in the file; `matches` lists every matching rule in order.
- String values in `payload` longer than `max_value_len` characters are cut and end with `…[truncated, N chars]`. Keys and structure are never dropped. `max_value_len = 0` keeps everything, which is how you capture real payloads when a new agent version arrives.
- If stdin wasn't usable, `payload` is the raw text (truncated the same way) and an `error` field says why.
- `ts` is when the hook started, with the local offset.

## Worked examples

### Claude: allow a safe command, with a shell-chaining safety net

```toml
[patterns]
shell_chain = ';|\||`|&&|&[^0-9]|&$|\$\('

[[rule]]
decision = "allow"
tool = "Bash"
description = "cargo workflow"
match."tool_input.command" = { regex = '^cargo (build|test|check|clippy|fmt|run)\b', not_regex = "@shell_chain" }
```

`cargo test` is allowed. `cargo test && curl evil.example` makes the rule not match, so Claude asks you as usual.

### Claude: limit writes to a directory, and deny secrets

```toml
[[rule]]
decision = "allow"
tool = "Write|Edit"
description = "edits inside the project"
match."tool_input.file_path" = { under = ["{cwd}"] }

[[rule]]
decision = "deny"
tool = "Read|Write|Edit"
reason = "Secrets files are off limits"
match."tool_input.file_path" = { regex = '\.(env|secret)$' }
```

Editing `{cwd}/.env` matches both rules; deny wins, whatever the order.

### Copilot: the same idea

```toml
[[rule]]
decision = "allow"
tool = "bash"
description = "cargo workflow"
match."toolArgs.command" = { regex = '^cargo (build|test|check)\b', not_regex = "@shell_chain" }
```

Copilot tool names are lowercase and arguments live under `toolArgs`. (The `toolArgs` field names are unverified; see [Copilot payloads](./copilot-tool-inputs.md).)

### A whole workflow

[`examples/mermaid-claude.toml`](../examples/mermaid-claude.toml) is a tightly scoped, illustrative config for generating Mermaid diagrams: an allow-list of exact commands and paths, with `@shell_chain` and `@parent_dir` safety nets. Everything else falls through to the normal prompt.

## Registering the hook

Registration is in the [README](../README.md#registering-the-hook): user level (`~/.claude/settings.json`, `~/.copilot/hooks/tool-gate-hook.json`) or per project, each with its own config file.
