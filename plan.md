# tool-gate-hook: Implementation Plan

Implements `spec.md`. Work happens on branch `rework-for-copilot` (local only; push and PR at phase 10).

## How to work through this plan

- Do steps in order. Each step is **test-first**: write that step's acceptance tests, watch them fail, implement, make them pass. If tests pass at once, break the code on purpose to prove they can fail.
- A step is done when its **Verify** list passes **and** the quality gate passes:
  ```bash
  cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
  ```
- Stop at the end of each step for the user to review. On approval, tick the box here and make one commit for the step (attribution line per the session's instructions).
- If a step shows that the spec is wrong or unclear, raise it with the user and update `spec.md` before continuing. Small decisions made along the way also go into `spec.md` once approved.
- Record decisions and context in `spec.md`, `plan.md` or `AGENTS.md`, never in agent memory files.
- **Coverage goal: enough to be confident it works, not exhaustive.** Test main behaviours and security-relevant edge cases; skip unlikely runtime edge cases, especially where a test adds complexity. One representative case per behaviour is enough.
- Shell note: macOS `sed` needs `-E` for alternation (`\|` doesn't work in basic regex).

## Current state (after step 7.1)

Both agents are supported end to end: config loading, all six matchers, tiered decisions, Claude and Copilot adapters, error handling and auditing (records are written by `main`). What remains is examples and docs (7), and verification against the real agents (8, 9).

### Modules as built

| Module | Contents |
|---|---|
| `main.rs` | clap CLI (`run`/`validate`, `Target { agent, config }`). `run` builds `Context::new(home)` from `std::env::home_dir()`, reads stdin, calls `lib::run`, prints `warnings` to stderr, appends `outcome.audit` with `auditing::append` (a failure is only a stderr warning) and prints `output` to stdout; **always exits 0**. `validate` loads the config, prints `validate::validate`'s summary to stdout and warnings (field paths unknown to the agent) to stderr, exits 1 on error. Imports anyhow's trait as `Context as _` to avoid clashing with `tool_gate_hook::Context`. |
| `lib.rs` | `Context { home, clock: fn() -> DateTime<FixedOffset> }` with `Context::new(home)` using the local clock (tests override `clock`); `Outcome { output: Option<Value>, warnings: Vec<String>, audit: Option<(PathBuf, AuditRecord)> }`; `run(agent, config_path, stdin, &Context) -> Outcome`. Order: parse JSON and `agent.parse()` first (failure → passthrough + warning, plus an error audit record if the config loads quietly), then `Config::load` (failure → `ask` with `tool-gate-hook config error (<path>): <details>`, no audit), then evaluate, build the audit record and render. |
| `agent.rs` | `Agent { Claude, Copilot }` (clap `ValueEnum`); `name()`; `default_config_path(home)`; `parse(&Value) -> Result<ToolCall>` (Claude: `tool_name`, `cwd`, rejects a non-`PreToolUse` `hook_event_name`; Copilot: `toolName`, `cwd`); `render(Decision, reason) -> Value`; `ToolCall { tool_name, cwd }`. Missing-field errors name the agent. |
| `config.rs` | `Config { audit: Option<AuditConfig>, policy }`, `Config::load(path)` / `from_toml(str)`. `AuditConfig { file, level, max_value_len }`, `AuditLevel { Off, Matched, All }`. Each `[[rule]]` is parsed from a `toml::Table` separately so errors read `rule #N (description): match."path": <matcher>: …`. `deny_unknown_fields` everywhere. `OneOrMany` for `regex`/`not_regex`; `@name` resolves `[patterns]`. Load errors don't include the path — callers add it. |
| `policy.rs` | `Decision { Allow, Ask, Deny }` (`Ord`, serde lowercase, `as_str`); `Policy { rules }`; `Rule { index (1-based), decision, tool: Option<Regex> (anchored), description, reason, fields }` with `reason()` (custom or `tool-gate-hook: <decision> by rule #N (description)`); `FieldCondition { path, matchers }`; `FieldMatcher { Regex, NotRegex, Equals, Glob, Under(Vec<String>), Exists }`; `Policy::evaluate(&payload, &ToolCall, &Context) -> Evaluation { matches: Vec<&Rule> }` with `decided_by()`. Values match as text (strings; numbers/bools as JSON text); null/arrays/objects/missing fail every matcher except `exists`. |
| `paths.rs` | `expand_dir(dir, home, cwd)` (`~`, `~/…`, `{cwd}`), `resolve(path, cwd)` (canonicalise longest existing prefix of the raw path, then clean up the remainder textually). Private module. |
| `auditing.rs` | `AuditRecord` (serialised as the spec's JSONL record, plus an optional `error`), built by `for_evaluation` (level filtering: `off` nothing, `matched` only if a rule matched, `all` everything) or `for_error` (raw stdin as a truncated string; every level but `off`); `Invocation { agent, config_path, started, finished }`; `truncate_json_strings` (marker `…[truncated, N chars]`, `0` disables); `append(file, &record)` (create+append, `File::lock`, one line; does not create parent directories). `ts` is the start-of-run clock read, `duration_us` the difference of two reads. |

### Tests and fixtures

- `tests/config.rs`: config parsing and error messages via `Config::from_toml`.
- `tests/claude.rs`: in-process acceptance tests. Helpers: `fixture(name)`, `with_field(name, &[path], json)`, `run_claude(config, stdin) -> Option<Value>`, `run_claude_outcome(...) -> Outcome`, `no_home()`, `decision(&output)`, `reason(&output)`, and `path_fixture()` / `run_under(...)` (temp tree with a symlink escaping the project).
- `tests/copilot.rs`: Copilot output shape, lowercase tool names, `toolArgs` rules, config error as `ask`, and payload mismatches in both directions (small helpers copied rather than shared).
- `tests/audit.rs`: whole-record comparisons with a fixed clock, level filtering, truncation and error records.
- `tests/smoke.rs`: spawns the binary with `HOME` set to a temp dir; covers missing `--agent`, default config path, exit 0 with `ask` on missing config, exit 0 on garbage stdin, Copilot variants of the missing-config and mismatched-payload cases, the real audit file (two runs → two lines, an unwritable path only warns), `validate` failure.
- `tests/fixtures/claude/{bash,read,write,edit,agent}.json`: current documented Claude payload shape (includes `permission_mode`, `effort`, `tool_use_id`, `prompt_id`).
- `tests/fixtures/copilot/{bash,view,create,edit,glob,task}.json`: documented top-level shape; the `toolArgs` field names are unverified (see the README there; confirmed in 9.2).

### Interim files (replaced later)

- `examples/{claude,copilot,mermaid-claude}.toml` are the example configs (tested in `tests/examples.rs`); the docs rewritten in 7.2 still link to the deleted `example.toml`.
- `README.md`, `docs/configuration-guide.md`, `docs/tool-input-schemas.md`, `tests/README.md` still describe the old design (rewritten in 7.2). `AGENTS.md` has an interim code-structure section pointing at `spec.md`.
- `docs/review-findings.md`: pre-rework findings; bug numbers there are referenced from steps below.

### Dependencies

`anyhow`, `clap`, `serde`, `serde_json`, `toml` 1.x, `regex`, `globset`, `chrono`, `log`, `env_logger`; dev: `pretty_assertions`, `tempfile`. Lints live in `[lints]` in `Cargo.toml`; never add `#![…]` lint attributes to source files.

### Verifying with a real agent without disrupting daily use

Claude verification (phase 8) uses a **scratch project** with the hook registered in that project's `.claude/settings.local.json`, not user-level settings, so a broken build only affects that project.

---

## Checklist

### Done

- [x] **0.1** Removed unused deps and `nix` (std `File::lock`), bumped everything to latest.
- [x] **0.2** `docs/review-findings.md`.
- [x] **1.1** Renamed to `tool-gate-hook`; lints moved to `Cargo.toml`.
- [x] **1.2** Required `--agent`, default config `~/.config/tool-gate-hook/<agent>.toml`, `run` always exits 0 (clap argument errors still exit 2, deliberately).
- [x] **2.1** New config model compiled to a `Policy`.
- [x] **2.2** Evaluation and Claude adapter.
- [x] **2.3** Error handling (`Outcome`, config error → `ask`, bad payload → passthrough).
- [x] **3.1** `equals`, `exists`, `glob`.
- [x] **3.2** `under`, with symlink-safe resolution (the spec's original lexical-first algorithm was unsafe and has been corrected).

### Phase 4: Copilot

- [x] **4.1 Copilot adapter**
  - Copilot parse: `toolName`, `cwd`. Copilot render: flat `{permissionDecision, permissionDecisionReason}`.
  - The reason is always present on deny, and default reasons are always produced.
  - Mismatch detection works both ways.
  - Add `tests/fixtures/copilot/` covering `bash`, `view`, `create`, `edit`, `glob`, `task`. Use the documented shape, with best-guess `toolArgs` field names marked as unverified in a fixture README note.
  - Starting point: replace the `bail!` in `Agent::parse`'s Copilot arm (`toolName`, `cwd` via `string_field`); make `string_field`'s error name the agent. `render`'s Copilot arm already exists. `smoke.rs`'s `BASH_PAYLOAD` is Claude-shaped, so add a Copilot one for the Copilot smoke test.
  - Documented Copilot input (native camelCase `preToolUse`): `{ sessionId, timestamp (Unix ms), cwd, toolName, toolArgs (parsed object) }`; tool names `bash`, `view`, `create`, `edit`, `glob`, `grep`, `rg`, `task`, `web_fetch`, … Output: `{ permissionDecision: allow|deny|ask, permissionDecisionReason }`. Source: https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-hooks-reference
  - **Verify:** acceptance tests show:
    - allow, deny and ask output for Copilot
    - config error giving Copilot-format `ask`
    - a Claude-shaped payload under `--agent copilot` giving passthrough
    - lowercase tool names matching
    - a rule on `toolArgs.command`
    - smoke tests for exit code 0 under Copilot

### Phase 5: Auditing

- [x] **5.1 New audit record**
  - The spec's JSONL record: `ts` from the clock, `agent`, `config` (canonical path), `decision` including `passthrough`, `decided_by`, `matches`, `payload`, `duration_us`.
  - Levels `off`, `matched` and `all`.
  - Add `clock` to `Context` (built as a plain `fn() -> DateTime<FixedOffset>`, not a trait: production uses local time via `Context::new`, tests a fixed time) and `audit: Option<(PathBuf, AuditRecord)>` to `Outcome`; `main` writes it in 5.3 (`ts` is the start-of-run clock read). `duration_us` = difference of two clock reads (0 in tests).
  - `matches` / `decided_by` entries are `{ index, decision, description }` built from `Evaluation`'s `&Rule`s.
  - **Verify:** acceptance tests compare whole records with a fixed clock for allow, deny, ask, passthrough and multi-match cases, plus level filtering (passthrough is not recorded at `matched` but is at `all`, and `off` records nothing).

- [x] **5.2 Truncation and error records**
  - Truncated values get the marker `…[truncated, N chars]`, and `max_value_len = 0` disables truncation.
  - Error records hold the raw stdin as a truncated string plus `error`, and they are written at both `matched` and `all`.
  - `run` checks the payload before loading the config, so for a mismatched or malformed payload it must still load the config quietly to find the audit settings (spec, Error handling). If that load fails, there is no record and no `ask`, just the stderr warning.
  - Start from the existing `auditing::truncate_json_strings` (currently appends a bare `…`, no length).
  - **Verify:** unit tests for truncation (nested, arrays, non-strings unchanged, char counting rather than bytes, the marker contents). Acceptance tests show a long `Write` content being truncated while all keys are kept, `0` keeping the full content, and malformed and mismatched payloads producing error records.

- [x] **5.3 Audit writing**
  - `audit::append` uses `OpenOptions` append and `File::lock`, writing one line per record.
  - Write failures produce a stderr warning only.
  - **Verify:** a smoke test runs twice against the same audit file and finds two valid JSON lines. An unwritable audit path still gives the correct decision on stdout and exit code 0. No concurrency test: two configs sharing one log file is rare, and `flock` is trusted.

### Phase 6: `validate`

- [x] **6.1 Validate command**
  - Print a summary to **stdout** (today it's an `info!` log that is invisible by default — review-findings bug #7): rule counts per decision, the pattern names, and the audit file, level and `max_value_len`.
  - Add the known top-level keys per agent to `agent.rs` (Claude: `session_id`, `prompt_id`, `transcript_path`, `cwd`, `scratchpad_dir`, `permission_mode`, `effort`, `hook_event_name`, `agent_id`, `agent_type`, `tool_name`, `tool_input`, `tool_use_id`; Copilot: `sessionId`, `timestamp`, `cwd`, `toolName`, `toolArgs`). Warnings are not errors (exit 0).
  - Exit non-zero on any config error.
  - **Warn** when a field path's first segment isn't one of the agent's known top-level keys.
  - **Verify:** smoke tests cover a valid config (exit 0 and the summary text), a broken config (non-zero exit and the error text), and a Claude config validated with `--agent copilot` (exit 0 with warnings naming each offending rule).

### Phase 7: Examples and docs

- [x] **7.1 Example configs**
  - Write `examples/claude.toml`, `examples/copilot.toml` and `examples/mermaid-claude.toml`. The mermaid one is illustrative only.
  - Each has commented `shell_chain` and `parent_dir` patterns.
  - Delete `example.toml` and `sample-mermaid-hook.toml`.
  - **Verify:** a test validates every file in `examples/` with the right agent with no warnings, and the mermaid scenario acceptance tests run against `examples/mermaid-claude.toml`.

- [ ] **7.2 Docs rewrite**
  - Rewrite `README.md` and `docs/configuration-guide.md`.
  - Split `docs/tool-input-schemas.md` into `docs/claude-tool-inputs.md` and `docs/copilot-tool-inputs.md`, with the Copilot one marked unverified until phase 9.
  - Rewrite `tests/README.md`, including the `jq` capture one-liner.
  - Rewrite `AGENTS.md` in full, keeping the testing-coverage guidance, the "Project knowledge" section and the `[lints]` note. Its "Logging" section (TOML `log_level`, trace/debug/info rule logging) describes things that don't exist; replace it with the real `RUST_LOG` stderr diagnostics and the audit log.
  - Document the `@` escaping rule (`\@` or `[@]`), `glob` being path-style, and `exists = false`.
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
