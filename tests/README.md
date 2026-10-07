# Tests

```bash
cargo test
```

The gate for every change is:

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

## Layout

| File | What it covers |
|------|----------------|
| `config.rs` | Config parsing and error messages (`Config::from_toml`) |
| `claude.rs` | Acceptance tests for the Claude agent: output shape, tiers and precedence, each matcher, `under` with symlinks and `..`, error handling |
| `copilot.rs` | The same for Copilot: flat output shape, lowercase tool names, `toolArgs` rules, mismatched payloads in both directions |
| `audit.rs` | Audit records with a fixed clock, level filtering, truncation, error records |
| `shell.rs` | Shell commands for both agents: segments, command rules, `paths_under`, `cd`, construct rules, each floor beating a broad allow and losing to a deny, config errors |
| `shell_differential.rs` | Our word splitting against `zsh -f` and `/bin/bash` (skipped if a shell is missing); commands the shells disagree on must hit a floor |
| `shell_corpus.rs` | `fixtures/shell/corpus.jsonl`: sanitised real commands with expected segment names and floors. Plus an ignored summary of a local, gitignored corpus (`scripts/shell-corpus.sh` builds it from an audit log) |
| `explain.rs` | `explain` with a command or a payload / audit record, reasons, truncated records |
| `examples.rs` | Every file in `examples/` validates without warnings, with its summary; the examples' scenarios; the recipes in `examples/skills/` load |
| `common/mod.rs` | Shared helpers: loading a fixture, running the hook with a config in a temp directory |
| `smoke.rs` | A handful of tests that spawn the real binary: CLI wiring, exit codes, `validate` output, the real audit file |
| `fixtures/claude/`, `fixtures/copilot/` | Sample hook payloads |

Acceptance tests run in-process: they call `tool_gate_hook::run` with a fixture payload, a config written to a temp directory and an injected `Context` (temp home, fixed clock) and assert on the whole `Outcome`. Smoke tests set `HOME` to a temp directory so nothing touches your real config.

Coverage goal: enough to be confident it works, not exhaustive. Main behaviours and security-relevant edge cases; one representative case per behaviour.

## Fixtures

`fixtures/claude/` holds Claude Code payloads (`bash`, `read`, `write`, `edit`, `agent`, `subagent_handback`). `fixtures/copilot/` holds Copilot payloads (`bash`, `view`, `create`, `glob`, `rg`, `task`, `apply_patch`), and `fixtures/copilot_via_claude/` what Copilot sends to a `.claude/settings.json` hook (`bash`, `read`, `edit`). All have shapes captured from real agents, with placeholder values (paths, ids).

### Capturing a real payload

Set `level = "all"` and `max_value_len = 0` in the config's `[audit]` section, make the agent do the thing, then pull the payload out of the log with `jq`. For example, the first Bash call:

```bash
jq -c 'select(.payload.tool_name == "Bash") | .payload' /tmp/tool-gate-hook-claude.jsonl \
  | head -n 1 | jq . > tests/fixtures/claude/bash.json
```

For Copilot select on `.payload.toolName == "bash"` instead. Scrub anything private (paths, session ids) before committing a fixture, then rerun the tests.
