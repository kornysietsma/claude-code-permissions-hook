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
| `examples.rs` | Every file in `examples/` validates without warnings; the Mermaid example's scenarios |
| `smoke.rs` | A handful of tests that spawn the real binary: CLI wiring, exit codes, `validate` output, the real audit file |
| `fixtures/claude/`, `fixtures/copilot/` | Sample hook payloads |

Acceptance tests run in-process: they call `tool_gate_hook::run` with a fixture payload, a config written to a temp directory and an injected `Context` (temp home, fixed clock) and assert on the whole `Outcome`. Smoke tests set `HOME` to a temp directory so nothing touches your real config.

Coverage goal: enough to be confident it works, not exhaustive. Main behaviours and security-relevant edge cases; one representative case per behaviour.

## Fixtures

`fixtures/claude/` holds the documented current Claude Code payload shape (`bash`, `read`, `write`, `edit`, `agent`). `fixtures/copilot/` holds Copilot payloads (`bash`, `view`, `create`, `edit`, `glob`, `task`); its README notes that the `toolArgs` field names are unverified.

### Capturing a real payload

Set `level = "all"` and `max_value_len = 0` in the config's `[audit]` section, make the agent do the thing, then pull the payload out of the log with `jq`. For example, the first Bash call:

```bash
jq -c 'select(.payload.tool_name == "Bash") | .payload' /tmp/tool-gate-hook-claude.jsonl \
  | head -n 1 | jq . > tests/fixtures/claude/bash.json
```

For Copilot select on `.payload.toolName == "bash"` instead. Scrub anything private (paths, session ids) before committing a fixture, then rerun the tests.
