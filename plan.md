# tool-gate-hook: Implementation Plan

Implements `spec.md`. Work happens on branch `rework-for-copilot`, which is **pushed to `origin`** (a backup; it tracks `origin/rework-for-copilot`). Phases 0 to 9 are done and committed; what remains is the review and the PR (phase 10).

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

### Shell and environment notes

- The user's **global tool-gate-hook is live in Claude sessions** (`~/.claude/settings.json` registers `~/.cargo/bin/tool-gate-hook run --agent claude`; config `~/.config/tool-gate-hook/claude.toml`, audit log `~/.local/share/tool-gate-hook/claude.jsonl`). Its one rule denies `python`, `pip`, `pipenv`, `virtualenv` and `pyenv` as a command. It treats quoted text as a command boundary, so a Bash command (a heredoc, a commit message) with a line starting `python…` or `pip…` is blocked. Write such text to a file with the Write tool and use `git commit -F <file>`; use `uv run` for any Python.
- zsh quirks: `echo` expands `\n` (write JSON with the Write tool, not `echo`), unquoted `$VAR` isn't word-split (use arrays or inline flags), and a bare `=====` is an error. In `perl -pi -e 's|…|…|'`, a `|` inside the pattern or replacement breaks the substitution: use the Edit tool for text containing `|`. macOS `sed` needs `-E` for alternation.
- A safety check blocks `bash -c "$var"` with shell variables; spell commands out or write a small script file.
- The installed binary `~/.cargo/bin/tool-gate-hook` is built from step 8's `src/`. Step 10.1 changed `src/` without changing behaviour (dead dependencies removed, small refactors); rebuild with `cargo install --path .` after the merge.
- Leftovers outside the repo, safe to delete: the scratch project `~/Dropbox/prj/ai/tgh-verify/` (the phase 8 Claude run) and `~/tgh-copilot-results/` (the raw Copilot audit logs from phase 9, with the work machine's username and paths; don't commit them).

## Current state

All code is done and verified against real installs: both agents (Claude and Copilot), six matchers (`regex`, `not_regex`, `equals`, `glob`, `under`, `exists`), tiered decisions, error handling, auditing, `validate`. Examples and docs are written, with real payload shapes. The quality gate passes (all tests green).

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

### Tests, fixtures, docs, scripts

- `tests/`: `config.rs`, `claude.rs`, `copilot.rs`, `audit.rs`, `examples.rs` (also the mermaid scenarios and the legacy-python rule in both agents' examples), `smoke.rs` (real binary, `HOME` set to a temp dir). `tests/README.md` has the layout and the `jq` capture one-liner. `tests/common/mod.rs` holds the helpers shared by `claude.rs`, `copilot.rs` and `audit.rs`.
- Fixtures (real shapes, placeholder values): `tests/fixtures/claude/{bash,read,write,edit,agent,subagent_handback}.json`, `tests/fixtures/copilot/{bash,view,create,apply_patch,rg,glob,task}.json`, `tests/fixtures/copilot_via_claude/{bash,read,edit}.json`.
- `examples/{claude,copilot,mermaid-claude}.toml` (validated; mermaid and the `@legacy_python` deny rule are scenario-tested).
- Docs: `README.md`, `docs/configuration-guide.md`, `docs/claude-tool-inputs.md`, `docs/copilot-tool-inputs.md`, `docs/copilot-verification.md`, `tests/README.md`, `AGENTS.md`. `docs/review-findings.md` has the historical pre-rework review (it still quotes the old format deliberately) and the phase 10 review.
- `scripts/copilot-verify.sh` (`setup` / `report` / `push [-n]` / `teardown`) drives the work-machine verification; it is a bash script, not covered by tests (it was exercised by hand in a sandbox).

### Verified facts worth remembering

- **Claude Code 2.1.289:** the subagent tool is `Agent`; in auto mode a `SubagentHandback` pseudo-tool fires; `Glob` and `Grep` are absent by default on macOS, Linux and WSL (searches arrive as `Bash`); `scratchpad_dir` is a top-level field.
- **Copilot CLI (2026-10-04, a GPT model and Haiku 4.5):** the tool set varies by model (`create` with `path` and `file_text`, or `apply_patch` with a **string** `toolArgs`; `grep` arrives as `rg`; `glob` and `rg` use `paths`). Repo-level hooks run in the repo root, so a relative `--config` works. `allow` suppresses a prompt, and `deny`, `ask` and config-error `ask` show their reason. A deny from one hook can stop other hooks running. `.claude/settings.json` hooks run too, with Claude-format payloads (Copilot field names). Details: `docs/copilot-tool-inputs.md`, `spec.md`.
- **Untested and not planned:** Copilot's `edit` tool (never seen), Claude-format decisions from a `.claude` hook, and Copilot reading the user-level `~/.claude/settings.json`. The user doesn't run both agents on one machine and configures Copilot to ignore `.claude/`. `tool_input` for Claude tools other than `Bash`, `Read`, `Write`, `Edit`, `Agent` and `SubagentHandback` comes from community sources, not captures.

### Dependencies

`anyhow`, `clap`, `serde`, `serde_json`, `toml` 1.x, `regex`, `globset`, `chrono`; dev: `pretty_assertions`, `tempfile`. Lints live in `[lints]` in `Cargo.toml`; never add `#![…]` lint attributes to source files.

---

## Checklist

### Done (phases 0 to 9, all committed)

- [x] **0** Dependencies bumped, `nix` replaced by std `File::lock`; `docs/review-findings.md` written.
- [x] **1** Renamed to `tool-gate-hook`, lints in `Cargo.toml`; required `--agent`, default config `~/.config/tool-gate-hook/<agent>.toml`, `run` always exits 0 (clap argument errors still exit 2, deliberately).
- [x] **2** New config model compiled to a `Policy`; evaluation and Claude adapter; error handling (config error → `ask`, bad payload → passthrough).
- [x] **3** `equals`, `exists`, `glob`; `under` with symlink-safe resolution (the spec's original lexical-first algorithm was unsafe and has been corrected).
- [x] **4** Copilot adapter, fixtures and acceptance tests.
- [x] **5** Audit record with an injected clock and level filtering; truncation marker and error records; locked append from `main`.
- [x] **6** `validate` summary and unknown-field-path warnings.
- [x] **7** Example configs and the docs rewrite.
- [x] **8** Local Claude verification: real run in a scratch project, fixtures and docs corrected; the user-level hook is live with the legacy-python rule (also `examples/claude.toml`).
- [x] **9** Copilot verification: kit (`docs/copilot-verification.md`, `scripts/copilot-verify.sh`), two work-machine runs, real payloads folded into fixtures, tests, examples and docs.

### Phase 10: Review and PR

- [x] **10.1 Full senior-engineer review**
  - Review the whole codebase against the engineering-standards skill: clarity, minimal comments, pure functions, errors as values, no dead code, latest dependencies. Include `tests/` and `scripts/copilot-verify.sh`.
  - Fix the findings and update `docs/review-findings.md` with what was addressed.
  - Known loose ends to settle here: the README's "See LICENSE file" line but there is no `LICENSE` file; the repository directory and remote are still named `claude-code-permissions-hook`; `tests/copilot.rs` copies small helpers from `tests/claude.rs` (consider `tests/common/mod.rs` if it reads better); the legacy-python rule treats quoted text as a command boundary (a known limit of regex matching; decide whether to document it in the configuration guide).
  - Outcome (decided with the user): eight findings fixed, listed in `docs/review-findings.md`. `log`/`env_logger` removed as unused (no `RUST_LOG`); MIT licence; shared test helpers in `tests/common/mod.rs`; the regex limit documented. The repository rename is left for after the merge (below).
  - **Verify:** the quality gate passes, and `cargo outdated` is clean.

- [ ] **10.2 PR**
  - Push the branch (already on `origin`) and open a PR against `main` summarising the changes, the verification done on both agents, and anything remaining (the untested items above).
  - **Verify:** the user approves the PR description before it is created.

### After the merge (by the user)

- Rename the GitHub repository to `tool-gate-hook` (GitHub redirects the old URL) and update the `origin` URL; rename the local directory. Add `repository` to `Cargo.toml` then.
- Rebuild the installed hook: `cargo install --path .`.
- The `legacy_python` deny rule replaces the old Python-based check on both machines. At home it is already live (the only `PreToolUse` command hook besides the docs helper). On the work machine, install the binary, copy the pattern and rule from `examples/copilot.toml` (and `examples/claude.toml` if Claude Code is used there) into `~/.config/tool-gate-hook/`, register the hook, then remove the old check.
