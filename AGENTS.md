# AGENTS.md

Guidance for coding agents (Claude Code, GitHub Copilot, …) working in this repository.

## Project Overview

`tool-gate-hook` is a Rust `PreToolUse` hook for Claude Code and GitHub Copilot CLI (macOS, local CLI). It evaluates user-defined TOML rules against the raw hook payload to auto-allow, auto-deny or force an `ask` for tool calls, passing everything else through to the agent's normal permission flow, and it writes a JSONL audit log of every call (payload, matching rules, decision).

`spec.md` is the design and `plan.md` the implementation plan and progress (it is mid-rework on the `rework-for-copilot` branch). Start there for anything about intended behaviour.

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

- **src/lib.rs**: `run()` takes agent, config path, stdin and a `Context` (home directory, clock) and returns an `Outcome` (output JSON or passthrough, stderr warnings, an audit record). No I/O beyond reading the config.
- **src/main.rs**: the CLI (`run` / `validate`, required `--agent`) and all I/O: stdin, stdout, stderr and the audit file write. `run` always exits 0.
- **src/agent.rs**: the `Agent` enum (claude/copilot): default config path, payload parsing (tool name and cwd), output rendering, known top-level payload keys.
- **src/config.rs**: TOML parsing (`[audit]`, `[patterns]`, `[[rule]]`) and compilation to a `Policy`.
- **src/policy.rs**: compiled rules, field-path lookup, the matchers and evaluation; `Decision` (allow < ask < deny).
- **src/paths.rs**: `~` / `{cwd}` expansion and symlink-safe path resolution for `under`.
- **src/auditing.rs**: `AuditRecord` (evaluation and error records), value truncation and `append` (locked JSONL write).
- **src/validate.rs**: the `validate` summary and its warnings.

Docs: `README.md` (entry point), `docs/configuration-guide.md` (config reference), `docs/claude-tool-inputs.md` and `docs/copilot-tool-inputs.md` (payloads), `examples/` (example configs), `docs/review-findings.md` (the pre-rework review), `docs/copilot-verification.md` (the work-machine checklist, driven by `scripts/copilot-verify.sh`).

## Important Details

### Hook protocol and rule matching

- Both agents' input and output formats are in `spec.md` ("Agent adapters"). Rules always address the **raw payload** by dotted field path; the adapters only extract the tool name and `cwd`.
- Every rule is evaluated; the final decision is tiered deny > ask > allow, and no match means passthrough (no output). See `spec.md` ("Configuration" and "Decision logic").
- `run` must **always exit 0**: Copilot treats any non-zero exit as a deny. Errors become an `ask` (config errors) or a passthrough (bad payloads) plus stderr.

### Logging and auditing

- The **audit log** is the product's observability: JSONL at the `[audit]` `file`, with `level` `off | matched | all` and `max_value_len` truncation, written under `File::lock`. Details in `docs/configuration-guide.md`.
- **Diagnostics** go to stderr via `log`/`env_logger`, controlled only by `RUST_LOG` (default `warn`). Warnings from `run` (bad payload, config error, audit write failure) are printed to stderr by `main`.
- Audit failures are non-fatal and never change the decision.

### Code Standards

- Strict linting is configured in the `[lints]` section of `Cargo.toml` (`unsafe_code = "forbid"`, clippy all, rust_2018_idioms, rust_2024_compatibility, deprecated_safe). Do not add `#![…]` lint attributes to source files.
- Error handling via `anyhow::Result`; tests use `pretty_assertions`.
- All clippy warnings are errors.
- Rust 2024 edition.

### Testing

- Test-first for each plan step. Acceptance tests run in-process through `tool_gate_hook::run` with an injected `Context`; a handful of smoke tests spawn the binary. Unit tests are for small, fiddly pure functions.
- Integration test inputs are in `tests/fixtures/{claude,copilot}/`; see `tests/README.md`. Example configs are in `examples/` and are tested.
- Coverage goal: enough to be confident things work, not exhaustive. Test main behaviours and security-relevant edge cases; skip unlikely runtime edge cases, especially where a test adds complexity (concurrency, process orchestration, elaborate setup).

### Project knowledge

- Record decisions, preferences and context in `spec.md`, `plan.md` or this file, not in agent memory files, which don't travel between machines.

## This is a Rust project

For Rust work, use the `rust-apps` skill. For general engineering standards (clarity, minimal comments, pure functions, errors as values), use the `engineering-standards` skill.
