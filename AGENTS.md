# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is `tool-gate-hook`, a Rust project that implements a PreToolUse hook for Claude Code (and, in progress, GitHub Copilot CLI), providing granular control over which tools Claude can use. It features allow/deny rules with regex pattern matching, security exclusions for path traversal and command injection, and thread-safe logging of all tool use.

## Build and Test Commands

```bash
# Build the project
cargo build

# Build release version
cargo build --release

# Run the application (validate config)
cargo run -- validate --agent claude --config example.toml

# Run as hook (reads JSON from stdin)
cat tests/read_allowed.json | cargo run -- run --agent claude --config example.toml

# Run tests
cargo test

# Run a specific test
cargo test <test_name>

# Run tests with output
cargo test -- --nocapture

# Check code without building
cargo check

# Run clippy linter
cargo clippy

# Run clippy with warnings as errors
cargo clippy -- -D warnings

# Format code
cargo fmt
```

## Code Structure

Mid-rework (see `spec.md` and `plan.md`, which are the source of truth for the target design and progress):

- **src/main.rs**: CLI (`run` / `validate`, required `--agent`), all I/O; `run` always exits 0
- **src/agent.rs**: `Agent` enum (claude/copilot), default config paths
- **src/config.rs**: TOML parsing (`[audit]`, `[patterns]`, `[[rule]]`) and compilation to a `Policy`
- **src/policy.rs**: compiled rule types and `Decision` (allow < ask < deny)
- **src/hook_io.rs**, **src/auditing.rs**: legacy, replaced in plan steps 2.2 and 5.1

## Important Details

### Hook Protocol

See `spec.md` ("Agent adapters") for both agents' input and output formats.

### Rule Matching

See `spec.md` ("Configuration" and "Decision logic"): every rule is evaluated, the final decision is tiered deny > ask > allow, and no match means passthrough (no output).

### Logging
- Log level configured in TOML via `log_level` (trace, debug, info, warn, error). Default: `info`
- Can be overridden by `RUST_LOG` environment variable (e.g., `RUST_LOG=debug cargo run`)
- **Log levels**:
  - `trace`: Logs every rule evaluation and regex match attempt
  - `debug`: Logs rule matches and exclude pattern hits
  - `info`: Logs only final allow/deny decisions
- Tool use logged to file specified in config with flock for thread safety
- Logging is non-fatal - errors won't block tool execution

### Code Standards
- Strict linting configured in the `[lints]` section of `Cargo.toml` (`unsafe_code = "forbid"`, clippy all, rust_2018_idioms, rust_2024_compatibility, deprecated_safe) — do not add `#![…]` lint attributes to source files
- Tests use `pretty_assertions` for better diff output
- Error handling via `anyhow::Result`
- All clippy warnings treated as errors in CI

### Cargo Edition
- Uses Rust 2024 edition

### Testing
- Unit tests in each module
- Integration test inputs in `tests/` directory
- Example config in `example.toml`
- Coverage goal: enough to be confident things work, not exhaustive. Test main behaviours and security-relevant edge cases; skip unlikely runtime edge cases, especially where a test adds complexity (concurrency, process orchestration, elaborate setup)

### Project knowledge
- Record decisions, preferences and context in `spec.md`, `plan.md` or this file — not in agent memory files, which don't travel between machines

## This is a Rust project

For rust coding see @/Users/korny/prompts/rust.md

For general engineering principals see @/Users/korny/prompts/software_engineering.md
