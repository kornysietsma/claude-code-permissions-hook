# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a Rust project that implements a PreToolUse hook for Claude Code, providing granular control over which tools Claude can use. It features allow/deny rules with regex pattern matching, security exclusions for path traversal and command injection, and thread-safe logging of all tool use.

## Build and Test Commands

```bash
# Build the project
cargo build

# Build release version
cargo build --release

# Run the application (validate config)
cargo run -- validate --config example.toml

# Run as hook (reads JSON from stdin)
cat tests/read_allowed.json | cargo run -- run --config example.toml

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

- **src/main.rs**: Entry point with CLI (run/validate commands) using clap subcommands
- **src/config.rs**: TOML configuration loading and regex compilation
- **src/hook_io.rs**: JSON serialization/deserialization for hook protocol
- **src/matcher.rs**: Rule matching logic with deny-first precedence
- **src/logging.rs**: Thread-safe file logging using flock

## Important Details

### Hook Protocol

The hook reads JSON from stdin with this structure:
```json
{
  "session_id": "abc123",
  "tool_name": "Read|Bash|Task|etc",
  "tool_input": { "file_path": "...", "command": "...", etc }
}
```

And outputs decisions to stdout:
```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow|deny",
    "permissionDecisionReason": "..."
  },
  "suppressOutput": true
}
```

### Rule Matching

- **Deny rules** are checked first and take precedence
- **Allow rules** are checked second
- Rules support main regex and exclude regex patterns
- Supported tools: Read, Write, Edit, Glob (file_path), Bash (command), Task (subagent_type, prompt)
- No match = no output (passthrough to normal Claude Code permission flow)

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
- Strict linting enabled: `#![forbid(unsafe_code)]`, `#![warn(clippy::all)]`, `#![warn(rust_2018_idioms)]`, `#![warn(rust_2024_compatibility)]`
- Tests use `pretty_assertions` for better diff output
- Error handling via `anyhow::Result`
- All clippy warnings treated as errors in CI

### Cargo Edition
- Uses Rust 2024 edition

### Testing
- Unit tests in each module
- Integration test inputs in `tests/` directory
- Example config in `example.toml`

## This is a Rust project

For rust coding see @/Users/korny/prompts/rust.md

For general engineering principals see @/Users/korny/prompts/software_engineering.md
