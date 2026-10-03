# tool-gate-hook: Implementation Plan

Implements `spec.md`. Work happens on branch `rework-for-copilot` (local only; push and PR at phase 10).

## How to work through this plan

- Do steps in order. Each step is **test-first** where code changes: write that step's acceptance tests, watch them fail, implement, make them pass. If tests pass at once, break the code on purpose to prove they can fail.
- A step is done when its **Verify** list passes **and** the quality gate passes:
  ```bash
  cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
  ```
- Stop at the end of each step for the user to review. On approval, tick the box here and make one commit for the step (attribution line per the session's instructions).
- If a step shows that the spec is wrong or unclear, raise it with the user and update `spec.md` before continuing. Small decisions made along the way also go into `spec.md` once approved.
- Record decisions and context in `spec.md`, `plan.md` or `AGENTS.md`, never in agent memory files.
- **Coverage goal: enough to be confident it works, not exhaustive.** Test main behaviours and security-relevant edge cases; skip unlikely runtime edge cases, especially where a test adds complexity. One representative case per behaviour is enough.
- Shell notes: macOS `sed` needs `-E` for alternation (`\|` doesn't work in basic regex). zsh needs `--include='*.md'` quoted. `python3` is blocked by a hook (use `uv run`, or perl/sed).

## Current state (after step 7.2)

All code is done: both agents (Claude and Copilot) are supported end to end — config loading, six matchers (`regex`, `not_regex`, `equals`, `glob`, `under`, `exists`), tiered decisions, error handling, auditing, `validate`. Examples and docs are written. What remains is **verification against the real agents** (phases 8 and 9) and the final review and PR (phase 10). Verification can change fixtures, examples and docs, and code only if a payload turns out different from the documented shape.

### Modules

| Module | Contents |
|---|---|
| `main.rs` | clap CLI (`run`/`validate`, `Target { agent, config }`). `run` builds `Context::new(home)`, reads stdin, calls `lib::run`, prints `warnings` to stderr, appends `outcome.audit` with `auditing::append` (failure → stderr warning only) and prints `output` to stdout; **always exits 0**. `validate` prints the summary to stdout and warnings to stderr, exits 1 on a config error. |
| `lib.rs` | `Context { home, clock: fn() -> DateTime<FixedOffset> }` (`Context::new(home)` uses the local clock; tests override `clock`); `Outcome { output, warnings, audit: Option<(PathBuf, AuditRecord)> }`; `run(agent, config_path, stdin, &Context) -> Outcome`. Order: parse JSON and `agent.parse()` first (failure → passthrough + warning, plus an error audit record if the config loads quietly), then `Config::load` (failure → `ask` with `tool-gate-hook config error (<path>): <details>`, no audit), then evaluate, build the audit record, render. |
| `agent.rs` | `Agent { Claude, Copilot }`: `name()`, `default_config_path(home)`, `parse` (Claude `tool_name`/`cwd`, rejects non-`PreToolUse` `hook_event_name`; Copilot `toolName`/`cwd`), `render(Decision, reason)`, `top_level_keys()` (used by `validate`). |
| `config.rs` | `Config { audit, pattern_names, policy }`, `load`/`from_toml`. Each `[[rule]]` is parsed from a `toml::Table` separately so errors read `rule #N (description): match."path": <matcher>: …`. `deny_unknown_fields` everywhere. Load errors don't include the path; callers add it. |
| `policy.rs` | `Decision` (`Ord`: allow < ask < deny), `Rule` (`index` 1-based, `label()`, `reason()`), `FieldCondition`, `FieldMatcher`, `Policy::evaluate` → `Evaluation { matches }` with `decided_by()`. Values match as text; null/arrays/objects/missing fail every matcher except `exists`. |
| `paths.rs` | `expand_dir` (`~`, `{cwd}`) and symlink-safe `resolve` for `under` (private module). |
| `auditing.rs` | `AuditRecord` (+ optional `error`), `for_evaluation` / `for_error`, `Invocation`, `truncate_json_strings`, `append` (create+append, `File::lock`; does not create parent directories). `ts` is the start-of-run clock read. |
| `validate.rs` | `validate(agent, &Config) -> Validation { summary, warnings }`; warns per field whose first path segment isn't a known top-level key for the agent. |

### Tests, fixtures, docs

- `tests/`: `config.rs`, `claude.rs`, `copilot.rs`, `audit.rs`, `examples.rs`, `smoke.rs` (real binary, `HOME` set to a temp dir). See `tests/README.md` for the layout and the `jq` capture one-liner.
- `tests/fixtures/claude/{bash,read,write,edit,agent}.json`: documented Claude payload shape (`agent.json` assumes the subagent tool is named `Agent`).
- `tests/fixtures/copilot/{bash,view,create,edit,glob,task}.json`: documented top-level shape; **the `toolArgs` field names are guesses** (see the README there).
- `examples/{claude,copilot,mermaid-claude}.toml` (validated and, for mermaid, scenario-tested by `tests/examples.rs`).
- Docs: `README.md`, `docs/configuration-guide.md`, `docs/claude-tool-inputs.md`, `docs/copilot-tool-inputs.md`, `tests/README.md`, `AGENTS.md`. `docs/review-findings.md` is the historical pre-rework review (it still quotes the old format deliberately).

### Things still marked unverified (to settle in phases 8 and 9)

- Claude: whether the subagent tool is `Task` or `Agent` (`docs/claude-tool-inputs.md`, `tests/fixtures/claude/agent.json`); the per-tool `tool_input` fields in that doc.
- Copilot: every `toolArgs.*` field name (`docs/copilot-tool-inputs.md`, `tests/fixtures/copilot/`, `examples/copilot.toml`, a note in the README "Status" section); the working directory of repo-level hooks; what happens when a `.claude/settings.json` hook fires under Copilot; whether a relative `--config` works for project-level Copilot hooks.

### Dependencies

`anyhow`, `clap`, `serde`, `serde_json`, `toml` 1.x, `regex`, `globset`, `chrono`, `log`, `env_logger`; dev: `pretty_assertions`, `tempfile`. Lints live in `[lints]` in `Cargo.toml`; never add `#![…]` lint attributes to source files.

### Verifying with a real agent without disrupting daily use

Claude verification (phase 8) uses a **scratch project** with the hook registered in that project's `.claude/settings.local.json`, not user-level settings, so a broken build only affects that project.

---

## Checklist

### Done

- [x] **0.1–0.2** Dependencies bumped, `nix` replaced by std `File::lock`; `docs/review-findings.md` written.
- [x] **1.1–1.2** Renamed to `tool-gate-hook`, lints in `Cargo.toml`; required `--agent`, default config `~/.config/tool-gate-hook/<agent>.toml`, `run` always exits 0 (clap argument errors still exit 2, deliberately).
- [x] **2.1–2.3** New config model compiled to a `Policy`; evaluation and Claude adapter; error handling (config error → `ask`, bad payload → passthrough).
- [x] **3.1–3.2** `equals`, `exists`, `glob`; `under` with symlink-safe resolution (the spec's original lexical-first algorithm was unsafe and has been corrected).
- [x] **4.1** Copilot adapter, fixtures and acceptance tests.
- [x] **5.1–5.3** Audit record with an injected clock and level filtering; truncation marker and error records; locked append from `main`.
- [x] **6.1** `validate` summary and unknown-field-path warnings.
- [x] **7.1** Example configs (the mermaid `curl` rule gained `@shell_chain`/`@parent_dir` safety nets; the `echo` rule is anchored).
- [x] **7.2** Docs rewrite (verified by extracting and validating the docs' TOML blocks; the old `_regex =` grep check was narrowed because it also matched `not_regex =`).

### Phase 8: Local Claude verification

- [ ] **8.1 Real Claude Code run** (needs the user at the keyboard)
  - `cargo install --path .`
  - In a scratch project, register the hook in `.claude/settings.local.json` (command `tool-gate-hook run --agent claude`, or an absolute path if `~/.cargo/bin` isn't on the hook's `PATH`), with a config using `level = "all"`, `max_value_len = 0`, and one allow, one deny and one ask rule.
  - Run prompts that use Bash, Read, Write, Edit, Glob, Grep and a subagent.
  - **Verify:**
    - each rule produces the expected behaviour in the UI: auto-run, a blocked call with its reason, and a prompt
    - passthrough calls prompt normally
    - a deliberately broken config causes every call to prompt with the config error
    - the audit log contains every call
  - Capture representative payloads into `tests/fixtures/claude/` with the `jq` one-liner (scrub private paths and ids), and confirm or correct the `Task` → `Agent` naming and the `docs/claude-tool-inputs.md` fields. Update the fixtures, examples and docs if the shape differs, and remove the corresponding "unverified" notes.

- [ ] **8.2 Switch user-level hook (manual, optional)**
  - The user moves their real config to `~/.config/tool-gate-hook/claude.toml` and registers the hook in `~/.claude/settings.json`.
  - **Verify:** one normal working session with no surprises.

### Phase 9: Copilot verification (work machine)

The work machine is Apple Silicon (this one is Intel); both have Rust. Transfer by rsync over ssh, initiated **from** the work machine.

- [ ] **9.1 Verification kit**
  - Write `docs/copilot-verification.md`, covering:
    - rsync commands (pulled from the work machine)
    - `cargo install --path .`
    - the user-level `~/.copilot/hooks/tool-gate-hook.json`
    - a test repo with `.github/hooks/tool-gate-hook.json`, a `.github/hooks/tool-gate-hook.toml`, and a `.claude/settings.json` hook to observe cross-reading
    - the verification config (`level = "all"`, `max_value_len = 0`, one allow, one deny and one ask rule)
    - a numbered prompt script exercising `bash`, `view`, `create`, `edit`, `glob`, `task`, plus one prompt per rule
    - what to rsync back
  - **Verify:** a dry read-through with the user. All commands work on this machine where possible (the build, `validate` of the verification configs).

- [ ] **9.2 Run on the work machine and fold results in**
  - The user runs the kit and rsyncs back the audit logs.
  - Turn the payloads into `tests/fixtures/copilot/`, then fix the `toolArgs` field names in the fixtures, examples and docs.
  - Record the repo-hook cwd finding, the relative `--config` finding and the `.claude/` cross-reading finding in the docs (README, `docs/copilot-tool-inputs.md`), and adjust the project-level registration instructions if needed.
  - **Verify:** the quality gate passes with the real fixtures, and the "unverified" markers are removed. A second work-machine run is needed only if code changed in a way the fixtures can't cover.

### Phase 10: Review and PR

- [ ] **10.1 Full senior-engineer review**
  - Review the whole codebase against the engineering-standards skill: clarity, minimal comments, pure functions, errors as values, no dead code, latest dependencies.
  - Fix the findings and update `docs/review-findings.md` with what was addressed.
  - Known loose ends to settle here: the README's "See LICENSE file" line but there is no `LICENSE` file; the repository directory and remote are still named `claude-code-permissions-hook`; `tests/copilot.rs` copies small helpers from `tests/claude.rs` (consider `tests/common/mod.rs` if it reads better).
  - **Verify:** the quality gate passes, and `cargo outdated` is clean.

- [ ] **10.2 PR**
  - Push the branch and open a PR against `main` summarising the changes, the verification done on both agents, and anything remaining.
  - **Verify:** the user approves the PR description before it is created.
