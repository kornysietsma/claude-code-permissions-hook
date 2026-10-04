# AGENTS.md

Guidance for coding agents (Claude Code, GitHub Copilot, …) working in this repository.

## Project Overview

`tool-gate-hook` is a Rust `PreToolUse` hook for Claude Code and GitHub Copilot CLI (macOS, local CLI). It evaluates user-defined TOML rules against the raw hook payload to auto-allow, auto-deny or force an `ask` for tool calls, passing everything else through to the agent's normal permission flow, and it writes a JSONL audit log of every call (payload, matching rules, decision).

## Build and Test Commands

```bash
cargo build                  # build (add --release for release)
cargo test                   # run all tests
cargo test <test_name>       # run one test
cargo test -- --nocapture    # show test output

# The quality gate: all three must pass
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test

# Validate a config
cargo run -- validate --agent claude --config examples/claude.toml

# Run as a hook (reads JSON from stdin)
cat tests/fixtures/claude/bash.json | cargo run -- run --agent claude --config examples/claude.toml
```

## Code Structure

- **src/lib.rs**: `run()` takes agent, config path, stdin and a `Context` (home directory, clock) and returns an `Outcome` (output JSON or passthrough, stderr warnings, an audit record). No I/O beyond reading the config (and canonicalising its path for the audit record).
- **src/main.rs**: the CLI (`run` / `validate`, required `--agent`) and all I/O: stdin, stdout, stderr and the audit file write. `run` always exits 0.
- **src/agent.rs**: the `Agent` enum (claude/copilot): default config path, payload parsing (tool name and cwd), output rendering, known top-level payload keys.
- **src/config.rs**: TOML parsing (`[audit]`, `[patterns]`, `[[rule]]`) and compilation to a `Policy`.
- **src/policy.rs**: compiled rules, field-path lookup, the matchers and evaluation; `Decision` (allow < ask < deny).
- **src/paths.rs**: `~` / `{cwd}` expansion and symlink-safe path resolution for `under`.
- **src/auditing.rs**: `AuditRecord` (evaluation and error records), value truncation and `append` (locked JSONL write).
- **src/validate.rs**: the `validate` summary and its warnings.

Docs: `README.md` (entry point), `docs/configuration-guide.md` (config reference), `docs/claude-tool-inputs.md` and `docs/copilot-tool-inputs.md` (payloads), `examples/` (example configs), `docs/review-findings.md` (the reviews before and after the rework), `docs/copilot-verification.md` (the work-machine checklist, driven by `scripts/copilot-verify.sh`).

## Important Details

### Hook protocol and rule matching

- Both agents' payloads and output formats are in `docs/claude-tool-inputs.md` and `docs/copilot-tool-inputs.md`. Rules always address the **raw payload** by dotted field path; the adapters only extract the tool name and `cwd`. Keep the payload as a `serde_json::Value`: a fixed struct drops new fields and breaks when agents change what they send.
- Every rule is evaluated; the final decision is tiered deny > ask > allow, and no match means passthrough (no output). The user-facing rules reference is `docs/configuration-guide.md`.
- `run` must **always exit 0**: Copilot treats any non-zero exit as a deny. Errors become an `ask` (config errors) or a passthrough (bad payloads) plus stderr. Invalid command-line arguments are the one exception: clap exits 2, deliberately blocking, since it only happens right after editing a hook registration.
- `--agent` is required with no auto-detection, because Copilot also runs `.claude/settings.json` hooks (with Claude-format payloads). The payload is checked **before** the config is loaded, so a payload for the other agent passes through quietly even when the config is broken.
- `under` canonicalises the longest existing prefix before applying the rest textually (`paths::resolve`). Cleaning up `..` textually first is unsafe with symlinks; keep it this way.
- Deliberately out of scope: parsing compound shell commands (allow rules use `not_regex` safety nets instead), rewriting tool input, hook events other than `PreToolUse`, merging user and project configs (the agents combine separate hooks most-restrictive-wins), built-in pattern presets, and compatibility with the old `[[allow]]`/`[[deny]]` format.

### Logging and auditing

- The **audit log** is the product's observability: JSONL at the `[audit]` `file`, with `level` `off | matched | all` and `max_value_len` truncation, written under `File::lock`. Details in `docs/configuration-guide.md`.
- **Diagnostics** are plain stderr lines (there is no `log` crate or `RUST_LOG`): warnings from `run` (bad payload, config error, audit write failure) are printed to stderr by `main`.
- Audit failures are non-fatal and never change the decision.

### Code Standards

- Strict linting is configured in the `[lints]` section of `Cargo.toml` (`unsafe_code = "forbid"`, clippy all, rust_2018_idioms, rust_2024_compatibility, deprecated_safe). Do not add `#![…]` lint attributes to source files.
- Error handling via `anyhow::Result`; tests use `pretty_assertions`.
- All clippy warnings are errors.
- Rust 2024 edition.

### Testing

- Test-first. Acceptance tests run in-process through `tool_gate_hook::run` with an injected `Context`; a handful of smoke tests spawn the binary. Unit tests are for small, fiddly pure functions.
- Integration test inputs are in `tests/fixtures/{claude,copilot,copilot_via_claude}/`: real captured shapes with placeholder values; see `tests/README.md` for capturing new ones from the audit log. Shared helpers are in `tests/common/mod.rs`. Example configs are in `examples/` and are tested.
- Real-agent checks are manual: for Copilot, `docs/copilot-verification.md` with `scripts/copilot-verify.sh` (not covered by tests); fold results back into fixtures and the payload docs.
- Coverage goal: enough to be confident things work, not exhaustive. Test main behaviours and security-relevant edge cases; skip unlikely runtime edge cases, especially where a test adds complexity (concurrency, process orchestration, elaborate setup).

### Project knowledge

- Record decisions, preferences and context in this file or `docs/`, not in agent memory files, which don't travel between machines.
- The author's own `tool-gate-hook` is live in their Claude sessions with the `legacy_python` deny rule, which also fires on quoted text. Bash commands with heredocs or commit messages containing a line that starts with a Python tool name get denied: write such text with a file tool and use `git commit -F <file>`.

## This is a Rust project

For Rust work, use the `rust-apps` skill. For general engineering standards (clarity, minimal comments, pure functions, errors as values), use the `engineering-standards` skill.
