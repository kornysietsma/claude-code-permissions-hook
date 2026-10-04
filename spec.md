# tool-gate-hook: Specification

## Goals

`tool-gate-hook` is a `PreToolUse` hook for **Claude Code** and **GitHub Copilot CLI** (macOS, local CLI only — not IDE or cloud agents). It replaces `claude-code-permissions-hook`.

It has two jobs:

1. **Observe** — log exactly what the agent sends to `PreToolUse` hooks, robust to payload changes in new agent versions, and record which rules matched, in order, and which one decided the outcome.
2. **Gate** — evaluate user-defined rules to auto-**allow**, auto-**deny**, or force an **ask** for tool calls; anything unmatched passes through to the agent's normal permission flow.

Backwards compatibility is explicitly **not** required: new name, new config format, new default config paths.

## Non-goals

- Copilot IDE integration or Copilot cloud agent (may come later)
- Hook events other than `PreToolUse` (no `PermissionRequest`, `PostToolUse`, no generic `log` subcommand — YAGNI)
- Rewriting tool input (`updatedInput` / `modifiedArgs`) or injecting `additionalContext`
- Parsing compound shell commands (`a && b`) — shell-chaining is handled by exclusion patterns that make an allow rule *not match*, so the call falls through to the user
- Merging user and project configs inside the tool (see "User and project level")
- Built-in pattern presets

## Work plan (phases)

| Phase | Content |
|---|---|
| 0. Light review | Update all dependencies to latest stable; fix anything clearly wrong; write findings (design lessons, smells, things to avoid) to `docs/review-findings.md` as input to the rework. Do **not** polish code that will be rewritten. Candidates already spotted: `lazy_static` → `std::sync::LazyLock`; check whether `derive_builder` and `itertools` still earn their place. |
| 1. Rename + CLI | Crate, binary, docs, `AGENTS.md` → `tool-gate-hook`; new CLI shape. |
| 2. Test-first throughout | Each implementation step starts by writing its own acceptance tests (see Testing), then implements until green. Detailed steps are in `plan.md`. |
| 3. Config + matching | New TOML model, named patterns, matchers, tiered decisions. |
| 4. Copilot adapter | The Copilot input/output adapter (the Claude one lands with phase 2). |
| 5. Auditing | New JSONL record, truncation, error records. |
| 6. `validate` | Summary on stdout, warnings for field paths foreign to the agent. |
| 7. Examples and docs | `examples/` configs and a rewrite of README and `docs/` per the Documentation section. |
| 8. Local Claude verification | Run under real Claude Code on this machine. |
| 9. Copilot verification | One batched session on the work machine (see below). |
| 10. Review and PR | Review the finished code against the engineering-standards skill, then open the PR. |

Push to a remote branch and open a PR near the end (phase 10, after both verifications).

## CLI

```
tool-gate-hook run      --agent claude|copilot [--config PATH]
tool-gate-hook validate --agent claude|copilot [--config PATH]
```

- `--agent` is **required** (no auto-detection — explicit is safer given Copilot reads `.claude/settings.json`).
- Default config: `~/.config/tool-gate-hook/<agent>.toml` (`claude.toml` / `copilot.toml`).
- A relative `--config` path resolves against the process working directory.
- `run` reads one JSON payload from stdin, writes a decision (or nothing) to stdout, always exits `0` (see Error handling).
- `validate` loads and compiles the config, prints a summary to **stdout** (rule count per decision, pattern names, audit file, level and `max_value_len`) and exits non-zero on any error. It also **warns** when a rule's `match` field path doesn't start with a top-level key known for that agent (e.g. a `claude` config using `toolArgs.path`) — a cheap guard against copy-pasting rules between agents. Warnings go to stderr, one per offending field, and are not errors (exit 0).

## Agent adapters

Each agent has a small adapter. Adapters only extract the few fields the engine itself needs and format the output; **rules always address the raw payload**.

### Claude Code

Input (stdin, snake_case). Relevant fields, current docs:
`session_id`, `prompt_id`, `transcript_path`, `cwd`, `permission_mode`, `effort.level`, `hook_event_name` (= `"PreToolUse"`), `agent_id`, `agent_type`, `tool_name`, `tool_input` (per-tool object, e.g. `command`, `file_path`), `tool_use_id`.

Adapter extracts: tool name = `tool_name`, cwd = `cwd`. Payload is a mismatch if `tool_name` or `cwd` is missing or not a string, or `hook_event_name` is present and not `PreToolUse`.

Output on a decision:

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow|deny|ask",
    "permissionDecisionReason": "..."
  },
  "suppressOutput": true
}
```

Passthrough = no stdout output, exit 0. (Claude also offers `"defer"`, which means the same thing — not used.)

Verified in phase 8 (Claude Code 2.1.289, and the official tools reference): the subagent tool is `Agent` (it was `Task` in earlier versions); in auto mode subagent completion fires a pseudo-tool `SubagentHandback`; `Glob` and `Grep` are absent by default on macOS, Linux and WSL (searches arrive as `Bash` calls) but present on Windows.

### Copilot CLI (native camelCase `preToolUse` format)

Input: `sessionId`, `timestamp` (Unix ms), `cwd`, `toolName` (`bash`, `view`, `create`, `edit`, `glob`, `grep`, `rg`, `task`, `web_fetch`, …), `toolArgs` (parsed object).

Adapter extracts: tool name = `toolName`, cwd = `cwd`. Payload is a mismatch if `toolName` or `cwd` is missing or not a string (e.g. Copilot sending the Claude-compatible snake_case payload because it picked up a `.claude/settings.json` hook).

Output on a decision:

```json
{ "permissionDecision": "allow|deny|ask", "permissionDecisionReason": "..." }
```

Passthrough = no stdout output, exit 0.

Copilot-specific behaviour the design relies on:
- **Any non-zero exit (other than timeout) is treated as deny** for `preToolUse` → `run` must always exit 0.
- Timeouts fail open.
- `permissionDecisionReason` is required on deny → we always send one.
- Copilot reads hooks from `.claude/settings.json` too. We avoid this where possible; docs tell Copilot users not to rely on `.claude/` hooks, and a mismatched payload passes through (see Error handling).

Unverified, to confirm on the work machine: exact `toolArgs` field names per tool, working directory for repo-level hooks, behaviour when a `.claude/settings.json` hook fires under Copilot.

## Configuration

TOML. Regexes should be written as **single-quoted literal strings** (no backslash doubling).

```toml
[audit]
file = "/tmp/tool-gate-hook-claude.jsonl"
level = "matched"        # off | matched | all
max_value_len = 1024     # truncate string values in logged payloads; 0 = never truncate

[patterns]
shell_chain = ';|\||`|&&|&[^0-9]|&$|\$\('
parent_dir  = '\.\.'

[[rule]]
decision = "allow"                     # allow | deny | ask   (required)
tool = "Bash"                          # regex, full-match on tool name; omit = any tool
description = "npx mermaid-cli mmdc"   # optional; used in logs and default reasons
reason = "..."                         # optional; sent to the model on deny/ask
match."tool_input.command" = { regex = '^npx -p @mermaid-js/mermaid-cli(@latest)? mmdc ', not_regex = "@shell_chain" }

[[rule]]
decision = "allow"
tool = "Read|Write"
description = "files under /tmp/mermaid"
match."tool_input.file_path" = { under = ["/tmp/mermaid"] }

[[rule]]
decision = "deny"
tool = "Read"
reason = "Secrets files are off limits"
match."tool_input.file_path" = { regex = '\.(env|secret)$' }
```

### Audit section

`[audit]` is optional; omitting it disables auditing. When present, `file` is required, `level` defaults to `matched` and `max_value_len` to `1024`.

### Rules

- `decision` — required, `allow | deny | ask`.
- `tool` — optional regex, anchored as `^(?:…)$` against the adapter-extracted tool name. Omitted = any tool.
- `description` — optional free text.
- `reason` — optional; returned as the decision reason. Default when absent: `tool-gate-hook: <decision> by rule #<index>[ (<description>)]`. Always present for deny/ask output.
- `match` — optional table keyed by **dotted field path** into the raw payload (e.g. `tool_input.command`, `toolArgs.path`, `cwd`, `permission_mode`, `agent_type`). Any payload field can be addressed. A rule with no `match` entries matches on `tool` alone.
- All `match` entries in a rule must pass (**AND**). OR = write separate rules.

### Field values

- Path lookup walks objects by key. A missing path = the matcher **fails** (rule doesn't match), except `exists = false`.
- String values are matched as-is; numbers and booleans are matched against their JSON text form (`120000`, `true`); objects/arrays only support `exists`.

### Matchers (per field; all given matchers must pass)

| Matcher | Type | Semantics |
|---|---|---|
| `regex` | string or list | Unanchored regex search (use `^`/`$` explicitly). With a list, passes if **any** item matches. |
| `not_regex` | string or list | Fails if **any** item matches. This is the exclusion / safety net: an excluded call makes the rule not match — it does **not** become a deny. |
| `equals` | string | Exact string equality. |
| `glob` | string | Glob match on the value (e.g. `**/*.rs`). Path-style: `*` does not cross `/`, `**` does. Intended for simple path checks — use `regex` for commands. |
| `under` | list of strings | Value is a path; passes if it lies inside any listed directory (see below). |
| `exists` | bool | Field present / absent. The only matcher that can pass when the field is missing (`exists = false`); every other matcher fails on a missing field. |

List items starting with `@` reference a `[patterns]` entry. Unknown names, invalid regexes or invalid globs are config errors (reported by `validate` and at load). An item starting with `@` is always a pattern reference; a regex that genuinely starts with `@` must be written `\@` or `[@]`. An empty matcher table (`match."x" = {}`) or empty list (`regex = []`) is a config error.

### `under` semantics

- Expansions in listed directories: `~` (alone or followed by `/`) → home directory, `{cwd}` → the adapter-extracted payload `cwd` (required in the payload; a payload without it is treated as not from this agent).
- Relative values are resolved against payload `cwd`.
- The longest **existing** prefix of the raw path is canonicalised first (following symlinks, with `..` applied the way the OS does), and only the not-yet-existing remainder is cleaned up textually (so not-yet-existing files for `Write`/`create` work). Cleaning up `..` textually first would be unsafe: with `/safe/link -> /etc/foo`, `/safe/link/../secret` looks like `/safe/secret` but the OS reaches `/etc/secret`.
- Containment is checked by path components, not string prefix (`/tmp/mermaid2` is not under `/tmp/mermaid`).
- Listed directories are canonicalised the same way.

### Named patterns

`[patterns]` maps names to regex strings. They are per config file (user and project configs do not share patterns). The example configs ship well-commented `shell_chain` and `parent_dir` definitions to copy. Every pattern is compiled at load, so a broken pattern is an error even if no rule uses it.

## Decision logic

1. Evaluate **every** rule in file order (no short-circuit) and collect all matches.
2. Final decision is tiered: **deny > ask > allow**, regardless of order.
3. The deciding rule is the first matching rule (in file order) with the winning decision; its reason is used.
4. No match → passthrough (no output).

## User and project level

Two independent hook registrations, each with its own config. The agents run all matching hooks and combine them most-restrictive-wins (Claude: deny > ask > allow > defer; Copilot: any deny blocks), so this is safe without any merge logic in the tool.

- Typical use (~95%): user level only.
- Project level is rare and opt-in, for projects with specific needs.

| | User level | Project level |
|---|---|---|
| Claude registration | `~/.claude/settings.json` | `.claude/settings.json` |
| Claude command | `tool-gate-hook run --agent claude` | `tool-gate-hook run --agent claude --config "$CLAUDE_PROJECT_DIR/.claude/tool-gate-hook.toml"` |
| Copilot registration | `~/.copilot/hooks/tool-gate-hook.json` | `.github/hooks/tool-gate-hook.json` |
| Copilot command | `tool-gate-hook run --agent copilot` | `tool-gate-hook run --agent copilot --config .github/hooks/tool-gate-hook.toml` (add explicit `cwd` in the hook entry if repo hooks don't run from repo root) |

Example registrations (docs must include complete versions):

```json
// ~/.claude/settings.json
{ "hooks": { "PreToolUse": [ { "matcher": "*", "hooks": [ { "type": "command", "command": "tool-gate-hook run --agent claude" } ] } ] } }
```

```json
// ~/.copilot/hooks/tool-gate-hook.json
{ "version": 1, "hooks": { "preToolUse": [ { "type": "command", "bash": "tool-gate-hook run --agent copilot", "timeoutSec": 10 } ] } }
```

Docs must note that `~/.cargo/bin` may not be on the hook's `PATH`; use an absolute path if needed.

Logging: each config names its own audit `file`. Pointing both at the same file is safe (one-line records written under `flock`); the `config` field distinguishes them.

## Auditing

JSON Lines, one record per invocation, appended under `flock`.

```json
{
  "ts": "2026-10-03T17:42:01.123+10:00",
  "agent": "claude",
  "config": "/Users/korny/.config/tool-gate-hook/claude.toml",
  "decision": "allow",
  "decided_by": { "index": 3, "description": "rm mermaid test files" },
  "matches": [ { "index": 3, "decision": "allow", "description": "rm mermaid test files" } ],
  "payload": { "...": "raw stdin JSON, string values truncated per max_value_len" },
  "duration_us": 412
}
```

- `decision` is `allow | deny | ask | passthrough`; `decided_by` is omitted on passthrough. Rule `index` is 1-based file order.
- `level`: `off` = nothing; `matched` = records where at least one rule matched, **plus all error records**; `all` = every invocation.
- Truncation: each string value longer than `max_value_len` chars is cut and suffixed with a marker containing the original length, e.g. `…[truncated, 53211 chars]`. JSON structure and all keys are always preserved. `max_value_len = 0` disables truncation (for debugging a new agent version). All audit settings live in the config file (reloaded every call), not on the command line.
- Error records: if stdin isn't valid JSON or doesn't match the `--agent` shape, `payload` is the raw stdin as a string (truncated) and an `error` field describes the problem (`stdin is not valid JSON: <detail>` or the adapter's message). Like other passthroughs they have `decision: "passthrough"` and `matches: []`.
- If the config couldn't be loaded, there is no audit destination; the error goes to stderr only (see below).
- Audit write failures go to stderr and never affect the decision.
- `ts` is the clock read at the start of the run, with the local UTC offset and millisecond precision; `duration_us` is the difference between a read at the start and one at the end. The clock is injected (a plain function in `Context`) for testability.
- `file` must be in an existing directory: the hook appends to the file (creating it) but does not create parent directories, so a bad path is only an audit write failure.

Diagnostic logging (`log`/`env_logger`, `RUST_LOG`) stays on stderr as today.

## Error handling

`run` **always exits 0** once its arguments parse. Copilot treats any non-zero exit as deny and Claude treats exit 2 as block, so exit codes are never used to signal decisions.

| Situation | Behaviour |
|---|---|
| Config error (missing explicit or default file, bad TOML, invalid regex/glob, unknown `@pattern`, unknown matcher key) | Output **`ask`** with reason `tool-gate-hook config error (<path>): <details>`, plus stderr. Loud but never locks you out — chosen because a config may break long after you've forgotten the hook exists. |
| Malformed stdin JSON / payload doesn't match `--agent` | Passthrough (no output), stderr warning, and an audit error record if the config loads (it is still loaded, quietly, just to find the audit settings — a config error here is not reported as `ask`). The payload is checked **before** the config is loaded, so a payload meant for another agent passes through quietly even when the config is broken — this hook isn't the one that should answer it. |
| Audit write failure | stderr only; decision unaffected. |
| Invalid command-line arguments (e.g. missing or unknown `--agent`) | clap error on stderr, **exit 2** — deliberately blocking (Claude treats exit 2 as block, Copilot as deny). This only happens right after editing a hook registration, so it surfaces immediately; without a valid `--agent` no well-formed `ask` can be produced anyway. |

## Testing

Test-first, per step: each step writes its acceptance tests before implementing (see `plan.md`).

- **Acceptance tests** (primary) run in-process: they call `tool_gate_hook::run` with a fixture payload, a config written to a temp dir and an injected `Context` (temp home; fixed clock), and assert on the whole `Outcome` (stdout JSON, warnings, audit record) with `pretty_assertions`.
- **Smoke tests** (a handful) spawn the real binary for CLI wiring, exit codes, `validate` and the real audit file write.
- **Coverage goal**: enough to be confident it works, not exhaustive — main behaviours plus security-relevant edge cases.
- Fixtures: `tests/fixtures/claude/…`, `tests/fixtures/copilot/…`. Initial fixtures come from documented payload examples; later, real captured payloads (via `level = "all"`, `max_value_len = 0`) are copied in. Document a `jq` one-liner for extracting a payload from an audit record into a fixture.
- The mermaid example config (see Documentation) doubles as a realistic acceptance scenario: a narrow workflow of allowed commands and paths, with shell-chain and `..` safety nets. It is illustrative only, so its rules don't need to match the user's current mermaid workflow.
- Must-cover cases: each decision tier and precedence (deny beats allow regardless of order), all matches recorded in order, `not_regex` exclusion → passthrough, `@pattern` lists, each matcher, `under` with `..`, symlinks, non-existent files, `{cwd}` and `~`; missing field; config error → ask (both agents' output formats); payload mismatch → passthrough + error record; truncation and `max_value_len = 0`; `validate` warnings.
- **Unit tests** only for small, fiddly pure functions: path normalisation/containment, field-path lookup, pattern resolution, value truncation.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` must pass.

## Copilot verification (work machine)

The work machine is Apple Silicon (this machine is Intel, so no cross-compiling); both have Rust. Transfer by rsync over ssh, initiated **from** the work machine (pull the source, push results back).

Do as much as possible before this from documentation fixtures. Then, in one batched session:

1. rsync the source; `cargo install --path .`
2. Register a user-level `preToolUse` hook and one repo-level hook (`.github/hooks/`).
3. Config with `level = "all"`, `max_value_len = 0`, and one allow, one deny and one ask rule.
4. Run a scripted list of prompts that exercise `bash`, `view`, `create`, `edit`, `glob`, `task`, plus one prompt per rule.
5. Also add a `.claude/settings.json` hook in the test repo to observe cross-reading behaviour.
6. rsync the audit log back; turn payloads into fixtures; confirm `toolArgs` field names, repo-hook cwd, and decision output handling; fix and re-test locally.

Phase 9 deliverable includes this checklist as a script or doc (`docs/copilot-verification.md`) so the session is mechanical.

## Documentation

The README is the entry point and stays short. Detailed reference material lives in `docs/`. Every existing doc is rewritten for the new design; nothing describing the old `[[allow]]`/`[[deny]]` format survives.

| File | Action |
|---|---|
| `README.md` | Rewrite: purpose, install, quick start for each agent side by side, user vs project setup, decision logic in brief, error behaviour, troubleshooting (PATH, config errors, Copilot reading `.claude/`). Link to `docs/` for detail. |
| `docs/configuration-guide.md` | **Rewrite from scratch** as the full config reference: `[audit]`, `[patterns]`, `[[rule]]` fields, every matcher with examples, field paths, `under` semantics, decision tiers, and worked examples for both agents (including the mermaid example). Drop the per-tool `*_regex` field sections. |
| `docs/tool-input-schemas.md` | **Split and rename** into `docs/claude-tool-inputs.md` and `docs/copilot-tool-inputs.md`. Each covers the full hook payload (top-level fields plus per-tool `tool_input` / `toolArgs`). Refresh the Claude doc from the current hooks docs and real captured payloads (e.g. `Task` → `Agent`, new top-level fields such as `permission_mode`, `agent_type`, `tool_use_id`). Write the Copilot doc from GitHub's docs and then correct it from the phase 9 captures. Keep the source-attribution table approach. State that the audit log (`level = "all"`, `max_value_len = 0`) is the authoritative source and the docs are a convenience snapshot, dated. |
| `docs/review-findings.md` | New (phase 0). |
| `docs/copilot-verification.md` | New (phase 9): the work-machine checklist. |
| `tests/README.md` | Rewrite for the new fixture layout and the `jq` capture workflow. Delete the old top-level `tests/*.json` fixtures and `tests/test_config.toml`. |
| `example.toml`, `sample-mermaid-hook.toml` | Replace with `examples/claude.toml`, `examples/copilot.toml` and `examples/mermaid-claude.toml`. Delete the old files. The mermaid file is an **illustrative** example of a tightly scoped workflow config (allow-list of commands and paths with `@shell_chain` / `@parent_dir` safety nets), loosely based on the old sample (with the safety nets applied to every command rule); it does not need to track the user's real workflow. |
| `update-thoughts.md` | Delete once the spec is accepted (superseded by this spec). |
| `AGENTS.md` | Update to the new name, structure, commands and docs layout. |
- Code standards: engineering-standards skill — minimal comments, pure functions where possible, `Result`-based errors, latest stable dependencies, Rust 2024 edition, existing lint settings.
