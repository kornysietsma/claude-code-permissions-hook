# tool-gate-hook: Implementation Plan

Implements `spec.md`. Work happens on branch `rework-for-copilot`.

## How to work through this plan

- Steps are done in order. Each step is **test-first**: write that step's acceptance tests, watch them fail, implement, make them pass.
- A step is done when its **Verify** list passes **and** the quality gate passes:
  ```bash
  cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
  ```
- Each completed step gets one commit, after the user has seen it. Check the box in this file in the same commit.
- If a step shows that the spec is wrong or unclear, stop and update `spec.md` (with the user) before continuing.

## Technical context and design decisions

### What survives from the current code

Little. The current engine is tied to per-tool `*_regex` fields and Claude-only payload structs. Things to keep or adapt:
- the `flock` append in `auditing.rs` (moving from `nix` to std `File::lock`, stable since Rust 1.89)
- the recursive `truncate_json_strings` (gains a length marker)
- the clap subcommand skeleton and lint attributes

Everything else is replaced. Old modules are deleted as soon as their replacement lands, so there is never a parallel old/new engine.

### Module layout (target)

| Module | Responsibility | Side effects |
|---|---|---|
| `main.rs` | clap CLI; builds the real `Context`; reads stdin; calls `run`/`validate`; prints output; writes audit; prints warnings to stderr; **always exits 0 for `run`** | all I/O |
| `lib.rs` | `run(agent, config_path, stdin, &Context) -> Outcome` and `validate(agent, config_path) -> Result<ValidationReport>`. Glues the modules together | reads the config file |
| `agent.rs` | `enum Agent { Claude, Copilot }`: `parse(&Value) -> Result<ToolCall, Mismatch>` (extracts tool name and cwd), `render(Decision, &str) -> Value` (output JSON), `known_top_level_keys()` (used by `validate`) | none |
| `config.rs` | serde structs for the TOML (`deny_unknown_fields`) and `compile() -> Result<Policy>`: resolves `@patterns` and compiles regexes and globs | none |
| `policy.rs` | compiled `Rule` and `FieldMatcher`, field-path lookup, `evaluate(&Policy, &Value, &ToolCall, &Context) -> Evaluation` (all matches plus the final decision) | `under` canonicalisation reads the filesystem |
| `paths.rs` | `under` support: expand `~` and `{cwd}`, lexical normalisation, canonicalising the longest existing ancestor, component-wise containment | reads the filesystem |
| `audit.rs` | `AuditRecord` construction, level filtering, value truncation, `append(path, &AuditRecord)` under lock | the write function only |

`Context` carries the injected dependencies: `clock: &dyn Clock` (`fn now(&self) -> DateTime<FixedOffset>`) and `home: PathBuf`. Production uses the local system clock and `std::env::home_dir()`. Tests use a fixed clock and a temp-dir home. `duration_us` is the difference between two clock reads, so with a fixed clock it is `0` in tests.

`Outcome` holds:
- `output: Option<serde_json::Value>`, the stdout JSON (None means passthrough)
- `audit: Option<(PathBuf, AuditRecord)>`, the record to write and where to write it (None when the level filters it out or the config failed to load)
- `warnings: Vec<String>`, which go to stderr

`run` returns this instead of performing I/O, so acceptance tests can assert on everything.

### Core types

```rust
enum Decision { Allow, Ask, Deny }            // Ord: Allow < Ask < Deny; final decision = max over matches
struct ToolCall { tool_name: String, cwd: PathBuf }
struct RuleMatch { index: usize, decision: Decision, description: Option<String> }
struct Evaluation { matches: Vec<RuleMatch>, decided_by: Option<RuleMatch> }   // None = passthrough
enum FieldMatcher { Regex(Vec<Regex>), NotRegex(Vec<Regex>), Equals(String), Glob(GlobMatcher), Under(Vec<String>), Exists(bool) }
```

The tiered decision falls out of `Ord`. `decided_by` is the first match, in file order, whose decision equals the maximum.

### Dependencies

| Keep / add | Purpose |
|---|---|
| `anyhow`, `clap` (derive), `serde`, `serde_json`, `toml`, `regex`, `chrono`, `log`, `env_logger` | as now, bumped to latest stable |
| `globset` (new) | `glob` matcher. Part of ripgrep, widely used and well maintained |
| `tempfile` (new, dev) | temp audit files, symlink trees for `under`, temp home |
| `pretty_assertions` (dev) | as now |

Remove: `lazy_static`, `derive_builder`, `itertools` (all unused) and `nix` (replaced by std `File::lock`).

### Testing approach

- **In-process acceptance tests** (`tests/acceptance_*.rs`) call `tool_gate_hook::run` with a fixture payload, a config, a fixed clock and a temp home, and assert on the whole `Outcome` (output JSON plus audit record) with `pretty_assertions`.
- **Configs** are written inline in the tests as TOML strings and saved to a temp dir. A config next to its assertions reads better than a separate file. The shared example configs under `examples/` also get tests that load them.
- **Payload fixtures** live in `tests/fixtures/{claude,copilot}/*.json`. They are files so that captured real payloads can be dropped in later.
- **Binary smoke tests** (`tests/smoke.rs`) spawn `env!("CARGO_BIN_EXE_tool-gate-hook")` for CLI wiring, the always-exit-0 rule, `validate` exit codes and a real audit file append. There are only a handful of them.
- **Unit tests** cover only fiddly pure functions: path normalisation and containment, field-path lookup, pattern resolution and truncation.
- **Coverage goal: enough to be confident it works, not exhaustive.** This is a personal project. Test the main behaviours and the security-relevant edge cases (precedence, exclusions, `under` traversal and symlinks). Skip unlikely runtime edge cases, especially when a test would add complexity such as concurrency, process orchestration or elaborate setup. The Verify lists below are guides; one representative case per behaviour is enough.

### Verifying with a real agent without disrupting daily use

Claude verification on this machine uses a **scratch project** with the hook registered in that project's `.claude/settings.local.json`, not the user-level settings. A broken build therefore only affects that project. Moving the hook to user level is a separate, final, manual step.

---

## Checklist

### Phase 0: Light review

- [x] **0.1 Dependency cleanup and update**
  - Remove `lazy_static`, `derive_builder`, `itertools`. Replace `nix` flock with std `File::lock` in `auditing.rs`.
  - Bump all remaining dependencies to their latest stable versions (`cargo outdated`, then edit `Cargo.toml`, then `cargo update`).
  - **Verify:** the quality gate passes with the existing tests unchanged, `cargo outdated` shows nothing outstanding, and `cat tests/read_allowed.json | cargo run -- run --config tests/test_config.toml` still prints an allow (with audit pointed at a temp file and level `all`, to exercise the new lock).

- [x] **0.2 Review findings**
  - Write `docs/review-findings.md` covering design lessons to carry forward and smells to avoid. Already seen:
    - `load_config` returns rules that `run_hook` discards
    - `process_hook_input_with_config` recompiles the rules
    - the `HookResult`/`Decision` split
    - Claude-only fields are hardcoded in `HookInput`
    - each tool's rule fields are duplicated
    - comments restate the code
    - the old tests go through the library API rather than the binary
  - Fix nothing that phases 1 to 5 will rewrite anyway.
  - **Verify:** the user reads and agrees with the findings.

### Phase 1: Rename and CLI

- [x] **1.1 Rename to `tool-gate-hook`**
  - Move the lint attributes into a `[lints.rust]` / `[lints.clippy]` section in `Cargo.toml` (`unsafe_code = "forbid"`, `rust_2018_idioms`, `rust_2024_compatibility`, `deprecated_safe`, `clippy::all`), and delete the `#![…]` attributes from every source file.
  - Change the package and binary name to `tool-gate-hook` and the library crate to `tool_gate_hook`. Update the `use` paths, the clap `about` text and `AGENTS.md` (name and commands only; the full rewrite is in 7.2).
  - **Verify:** the quality gate passes, and `cargo run -- validate --config example.toml` works under the new name.

- [x] **1.2 New CLI shape**
  - Make `--agent claude|copilot` required on `run` and `validate`, and `--config` optional.
  - The default config is `home/.config/tool-gate-hook/<agent>.toml`.
  - `run` always exits 0. Any error goes to stderr, and the call passes through until 2.3 adds the "ask" behaviour.
  - Start `tests/smoke.rs`.
  - **Verify:** smoke tests confirm that a missing `--agent` is a clap error, the default path resolves under the test `HOME`, and `run` with a nonexistent config exits 0.

### Phase 2: New engine (Claude only)

- [ ] **2.1 Config model and compilation**
  - New `config.rs` covering `[audit]` (`file`, `level`, `max_value_len` defaulting to 1024), `[patterns]`, and `[[rule]]` (`decision`, `tool`, `description`, `reason`, `match`).
  - In this step `match` supports only `regex` and `not_regex`, each taking a string or a list, with `@name` references.
  - Use `deny_unknown_fields` throughout, so a typo in a matcher key is an error.
  - Compile to a `Policy`. Anchor `tool` as `^(?:…)$`.
  - Errors name the rule index and field path.
  - Delete the old `[[allow]]`/`[[deny]]` structures.
  - **Verify:** tests cover a valid config and a few representative errors (an unknown `@pattern`, a bad regex, a misspelt matcher key), with the error message naming the offending rule.

- [ ] **2.2 Evaluation and Claude adapter (the `run` pipeline)**
  - `policy.rs` covers field-path lookup (missing path means no match; numbers and booleans match as JSON text; objects and arrays never match `regex`), the AND of all entries in `match`, evaluation of every rule, and the tiered decision.
  - `agent.rs` contains the Claude parse and render functions, with default reasons.
  - Add `Context`, `Clock` and `Outcome`, and `lib::run` wired into `main`. The audit record is still the old one or a stub, since it gets replaced in 5.1.
  - Delete the old `matcher.rs`, `hook_io.rs`, `tests/integration_test.rs` and `tests/*.json`, and add `tests/fixtures/claude/` (Bash, Read, Write, Edit, Agent payloads in the current documented shape).
  - **Verify:**
    - deny beats allow regardless of file order, and ask beats allow
    - `decided_by` is the first rule with the winning tier, and every match is recorded in file order
    - a `not_regex` hit gives passthrough, not deny
    - `@pattern` lists work; a missing field means no match
    - `tool` is anchored (`Read` doesn't match `ReadMcpResource`)
    - matching on a top-level field (`permission_mode`) works
    - the output JSON is exactly the spec shape for allow, deny and ask
    - a custom `reason` is used, and the default reason format is correct
    - manual check: `cat tests/fixtures/claude/bash.json | cargo run -- run --agent claude --config <tmp config>`

- [ ] **2.3 Error handling**
  - A config error (missing file, bad TOML, bad regex, unknown pattern) produces an `ask` with the reason `tool-gate-hook config error (<path>): <details>` and a stderr warning.
  - Malformed JSON, or a payload that doesn't match the agent, produces passthrough and a warning; its audit record arrives in 5.2.
  - `main` never exits non-zero for `run`.
  - **Verify:** acceptance tests cover each config error giving `ask` in Claude format, garbage stdin giving passthrough, and a Copilot-shaped payload under `--agent claude` giving passthrough. Smoke tests show exit code 0 in each case.

### Phase 3: Remaining matchers

- [ ] **3.1 `equals`, `exists`, `glob`**
  - Add `globset` and compile globs when the config loads; a bad glob is a config error.
  - **Verify:** acceptance tests show each matcher passing and failing, `exists = false` on a missing field matching, several matchers on one field being ANDed, and a bad glob producing a config error that becomes `ask`.

- [ ] **3.2 `under`**
  - `paths.rs` provides `~` expansion from `Context.home`, `{cwd}` from `ToolCall.cwd`, resolution of relative values against cwd, lexical normalisation, canonicalisation of the longest existing ancestor, and component containment. Listed directories are canonicalised the same way.
  - **Verify:**
    - unit tests in `paths.rs`
    - acceptance tests in a tempdir tree:
      - `/x/allowed/../secret` is not under `/x/allowed`
      - a symlink inside the allowed dir that points outside is not under it
      - a not-yet-existing file in an allowed dir is under it
      - `/tmp/mermaid2` is not under `/tmp/mermaid`
      - `{cwd}` and `~` expand
      - a relative `file_path` resolves against payload cwd

### Phase 4: Copilot

- [ ] **4.1 Copilot adapter**
  - Copilot parse: `toolName`, `cwd`. Copilot render: flat `{permissionDecision, permissionDecisionReason}`.
  - The reason is always present on deny, and default reasons are always produced.
  - Mismatch detection works both ways.
  - Add `tests/fixtures/copilot/` covering `bash`, `view`, `create`, `edit`, `glob`, `task`. Use the documented shape, with best-guess `toolArgs` field names marked as unverified in a fixture README note.
  - **Verify:** acceptance tests show:
    - allow, deny and ask output for Copilot
    - config error giving Copilot-format `ask`
    - a Claude-shaped payload under `--agent copilot` giving passthrough
    - lowercase tool names matching
    - a rule on `toolArgs.command`
    - smoke tests for exit code 0 under Copilot

### Phase 5: Auditing

- [ ] **5.1 New audit record**
  - The spec's JSONL record: `ts` from the clock, `agent`, `config` (canonical path), `decision` including `passthrough`, `decided_by`, `matches`, `payload`, `duration_us`.
  - Levels `off`, `matched` and `all`.
  - Replace the old `AuditEntry`.
  - **Verify:** acceptance tests compare whole records with a fixed clock for allow, deny, ask, passthrough and multi-match cases, plus level filtering (passthrough is not recorded at `matched` but is at `all`, and `off` records nothing).

- [ ] **5.2 Truncation and error records**
  - Truncated values get the marker `…[truncated, N chars]`, and `max_value_len = 0` disables truncation.
  - Error records hold the raw stdin as a truncated string plus `error`, and they are written at both `matched` and `all`.
  - **Verify:** unit tests for truncation (nested, arrays, non-strings unchanged, char counting rather than bytes, the marker contents). Acceptance tests show a long `Write` content being truncated while all keys are kept, `0` keeping the full content, and malformed and mismatched payloads producing error records.

- [ ] **5.3 Audit writing**
  - `audit::append` uses `OpenOptions` append and `File::lock`, writing one line per record.
  - Write failures produce a stderr warning only.
  - **Verify:** a smoke test runs twice against the same audit file and finds two valid JSON lines. An unwritable audit path still gives the correct decision on stdout and exit code 0. No concurrency test: two configs sharing one log file is rare, and `flock` is trusted.

### Phase 6: `validate`

- [ ] **6.1 Validate command**
  - Print a summary: rule counts per decision, the pattern names, and the audit file, level and `max_value_len`.
  - Exit non-zero on any config error.
  - **Warn** when a field path's first segment isn't one of the agent's known top-level keys.
  - **Verify:** smoke tests cover a valid config (exit 0 and the summary text), a broken config (non-zero exit and the error text), and a Claude config validated with `--agent copilot` (exit 0 with warnings naming each offending rule).

### Phase 7: Examples and docs

- [ ] **7.1 Example configs**
  - Write `examples/claude.toml`, `examples/copilot.toml` and `examples/mermaid-claude.toml`. The mermaid one is illustrative only.
  - Each has commented `shell_chain` and `parent_dir` patterns.
  - Delete `example.toml` and `sample-mermaid-hook.toml`.
  - **Verify:** a test validates every file in `examples/` with the right agent with no warnings, and the mermaid scenario acceptance tests run against `examples/mermaid-claude.toml`.

- [ ] **7.2 Docs rewrite**
  - Rewrite `README.md` and `docs/configuration-guide.md`.
  - Split `docs/tool-input-schemas.md` into `docs/claude-tool-inputs.md` and `docs/copilot-tool-inputs.md`, with the Copilot one marked unverified until phase 9.
  - Rewrite `tests/README.md`, including the `jq` capture one-liner.
  - Rewrite `AGENTS.md` in full, keeping the testing-coverage guidance and the "Project knowledge" section.
  - Delete `update-thoughts.md`.
  - **Verify:**
    - `grep -rn "allow\]\]\|deny\]\]\|_regex =\|claude-code-permissions-hook" --include=*.md .` returns no hits (outside `spec.md`/`plan.md`)
    - a manual pass checking that the TOML snippets in the docs match the implemented format
    - the user reads the README

### Phase 8: Local Claude verification

- [ ] **8.1 Real Claude Code run**
  - `cargo install --path .`
  - In a scratch project, register the hook in `.claude/settings.local.json`, and use a config with `level = "all"`, `max_value_len = 0`, and one allow, one deny and one ask rule.
  - Run prompts that use Bash, Read, Write, Edit, Glob, Grep and a subagent.
  - **Verify:**
    - each rule produces the expected behaviour in the UI: auto-run, a blocked call with its reason, and a prompt
    - passthrough calls prompt normally
    - a deliberately broken config causes every call to prompt with the config error
    - the audit log contains every call
  - Capture representative payloads into `tests/fixtures/claude/` with the `jq` one-liner, and confirm or correct the `Task` → `Agent` naming and the `docs/claude-tool-inputs.md` fields.

- [ ] **8.2 Switch user-level hook (manual, optional)**
  - The user moves their real config to `~/.config/tool-gate-hook/claude.toml` and registers the hook in `~/.claude/settings.json`.
  - **Verify:** one normal working session with no surprises.

### Phase 9: Copilot verification (work machine)

- [ ] **9.1 Verification kit**
  - Write `docs/copilot-verification.md`, covering:
    - rsync commands (pulled from the work machine)
    - `cargo install --path .`
    - the user-level `~/.copilot/hooks/tool-gate-hook.json`
    - a test repo with `.github/hooks/tool-gate-hook.json`, a `.github/hooks/tool-gate-hook.toml`, and a `.claude/settings.json` hook to observe cross-reading
    - the verification config
    - a numbered prompt script
    - what to rsync back
  - **Verify:** a dry read-through with the user. All commands work on this machine where possible (the build, `validate` of the verification configs).

- [ ] **9.2 Run on the work machine and fold results in**
  - The user runs the kit and rsyncs back the audit logs.
  - Turn the payloads into `tests/fixtures/copilot/`, then fix the `toolArgs` field names in the fixtures, examples and docs.
  - Record the repo-hook cwd finding and the `.claude/` cross-reading finding in the docs, and adjust the project-level registration instructions if needed.
  - **Verify:** the quality gate passes with the real fixtures, and the "unverified" markers are removed. A second work-machine run is needed only if code changed in a way the fixtures can't cover.

### Phase 10: Review and PR

- [ ] **10.1 Full senior-engineer review**
  - Review the whole codebase against the engineering-standards skill: clarity, minimal comments, pure functions, errors as values, no dead code, latest dependencies.
  - Fix the findings and update `docs/review-findings.md` with what was addressed.
  - **Verify:** the quality gate passes, and `cargo outdated` is clean.

- [ ] **10.2 PR**
  - Push the branch and open a PR against `main` summarising the changes, the verification done on both agents, and anything remaining.
  - **Verify:** the user approves the PR description before it is created.
