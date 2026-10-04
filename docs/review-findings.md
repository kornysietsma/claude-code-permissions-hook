# Review findings (pre-rework)

This is a light review of `claude-code-permissions-hook` at commit `acf5d88`, done before the `tool-gate-hook` rework. Its purpose is to carry lessons into the rework rather than fix code that is about to be replaced. Items marked **confirmed** were reproduced by running the binary.

## Behaviour bugs

| # | Finding | Status | Addressed by |
|---|---|---|---|
| 1 | **Typos in config keys are silently ignored.** `comand_regex = "rm"` in a deny rule passes `validate` and never matches, so a mistyped deny rule silently disappears. | confirmed | `deny_unknown_fields` |
| 2 | **A rule with no main regex never matches.** `[[deny]] tool = "Bash"` with no `command_regex` does nothing. An exclude regex without a main regex is ignored too. | by reading | Rules with no `match` entries match on `tool` alone |
| 3 | **Fields that don't apply to the rule's tool are silently ignored.** `command_regex` on a `Read` rule is accepted and never evaluated. | by reading | Field paths address the payload directly, and `validate` warns on unknown roots |
| 4 | **`Task` rules OR their conditions.** A rule with both `subagent_type` and `prompt_regex` matches if *either* matches, which is surprising for a deny or allow. | by reading | All `match` entries are ANDed |
| 5 | **A payload missing any expected field fails the hook with exit 1.** `HookInput` requires `session_id`, `transcript_path` and `hook_event_name`. Claude treats exit 1 as a non-blocking error, so the call fails open with only a stderr message. Under Copilot, exit 1 would mean deny. | confirmed | Payload kept as raw `Value`, mismatch passes through, always exit 0 |
| 6 | **Config errors fail open silently.** A missing or broken config makes `run` exit 1, so Claude proceeds through its normal flow and nothing visible tells you the hook is off. | by reading | Config error produces `ask` |
| 7 | **`validate` prints nothing.** Its summary is logged at `info!`, but the default log filter is `warn`, so a valid config produces no output at all. | confirmed | Print the summary to stdout |
| 8 | **Only hardcoded tools can be ruled on.** Tools are limited to `Read`/`Write`/`Edit`/`Glob`/`Bash`/`Task`. Rules for any other tool (Grep, WebFetch, MCP tools, `Agent`) are silently ignored, and tool matching is exact-string only. | by reading | `tool` regex plus field paths |

## Design lessons

- **Keep the raw payload.** Deserialising into a fixed `HookInput` struct both throws away unknown fields (`permission_mode`, `agent_type` and so on, which goes against the "observe" goal) and makes the hook brittle when fields are missing. The rework keeps `serde_json::Value` and extracts only tool name and cwd.
- **Compile once, pass the compiled form around.** `load_config` compiles the rules, `run_hook` throws them away (`let _ = (deny_rules, allow_rules)`), and `process_hook_input_with_config` compiles them again. The rework has one `compile() -> Policy` and one owner of it.
- **Per-tool fields multiply.** Each tool had its own `*_regex`/`*_exclude_regex` pair, duplicated across `RuleConfig`, `Rule`, `compile_rule` and `check_rule`. A generic `field → matchers` map removes all four copies.
- **Put types where they belong.** `Decision` lives in `auditing.rs` but is the core output of matching. `HookResult` and its trivial constructors duplicate `Decision` plus a reason.
- **Keep a small library API.** `lib.rs` exposes three near-identical entry points (`process_hook_input`, `load_config`, `validate_config`). The rework has two, `run` and `validate`.
- **Inject time.** `Utc::now()` is called inside the audit code, so audit records can't be asserted in tests. The rework injects it via `Context`.
- **Audit records need to explain decisions.** The current record has only a free-text reason, with no rule identity, no list of other matching rules, no config path and no agent. Truncation cuts silently at 256 chars with no indication of the original length.
- **Make reasons useful to the model.** The default reason echoes the full command (`Matched rule for Bash with command: …`), which tells the model nothing actionable. Per-rule `reason` text fixes this.
- **Regex exclusions are a weak path-safety tool.** A `\.\.` exclusion is heuristic and ignores symlinks. The `under` matcher replaces it.
- **Broad deny regexes catch harmless commands.** `tests/test_config.toml` denies any Bash command containing `&`, which also blocks `2>&1`. The rework steers people towards `not_regex` safety nets on allow rules, which fall through to asking, rather than broad deny rules.

## Code-level smells (do not repeat)

- **Comments restating code:** `// Check deny rules first`, `// Audit the decision`, `// Numbers, bools, null pass through unchanged`.
- **Lint attributes repeated in every module:** `#![forbid(unsafe_code)]` and `#![warn(clippy::all)]` in each submodule duplicate the crate-root attributes. Declare them once per crate root, or in `[lints]` in `Cargo.toml`.
- **Doc comments on non-public or self-evident items:** for example `/// Create an allow result with a reason.`
- **Tests that don't check behaviour:** `test_hook_result_constructors` tests struct literals, and the "example config is valid" tests depend on personal paths in `example.toml`.
- **Integration tests bypass the binary.** They call the library API directly, so CLI wiring, stdout format and exit codes are untested. The rework adds smoke tests that run the binary.

## Fixed before the rework

- Removed unused `lazy_static`, `derive_builder` and `itertools`. Replaced `nix` flock with std `File::lock`. All dependencies are at their latest stable versions.

Nothing else was fixed: every finding above was in code that the rework replaced.

# Review findings (final review, before the PR)

A senior-engineer review of the finished rework (`src/`, `tests/`, `scripts/copilot-verify.sh`, docs) against the engineering-standards skill. The quality gate passed and `cargo outdated` was clean before and after. Overall the code held up: a pure `run` with an injected `Context`, errors as values, no unwraps in production code, no dead modules, small functions, and comments only where something is surprising.

| # | Finding | Resolution |
|---|---|---|
| 1 | **Dead dependencies.** `log` and `env_logger` were initialised, but nothing called a `log` macro, so the README's "`RUST_LOG=debug` writes to stderr" was false. | Removed both and every `RUST_LOG` mention. Diagnostics are plain stderr lines; the audit log is the observability story. |
| 2 | **Missing licence.** The README said "See LICENSE file" but there never was one. | Added an MIT `LICENSE`, plus `license` and `description` in `Cargo.toml`. |
| 3 | **Duplicated rule label.** `config.rs` rebuilt the `rule #N (description)` text that `Rule::label` produces. | One `policy::rule_label`, used by both. |
| 4 | **Unneeded public API.** `Outcome::passthrough_with_warning` was public and used once; `Outcome` derived an unused `Default`. | Inlined the constructor, dropped the derive. |
| 5 | **Older clap attribute style.** `#[clap(...)]` throughout `main.rs`. | Now `#[command(...)]` / `#[arg(...)]`, the clap 4 names. |
| 6 | **Audit lock error lacked context.** A failed `File::lock` reported a bare OS error. | Wrapped as `cannot lock <file>`. |
| 7 | **Copied test helpers.** `tests/claude.rs`, `copilot.rs` and `audit.rs` each had their own fixture loader and temp-config runner. | Shared in `tests/common/mod.rs`. `examples.rs` and `smoke.rs` keep their own helpers, which differ. |
| 8 | **Regex matching sees text, not shell syntax.** The `legacy_python` deny rule fires on quoted text (heredocs, commit messages) that has a line starting with one of its commands. It fired on the shell command that first tried to write this table. | Documented in the configuration guide ("Regexes see text, not shell syntax") and next to the pattern in both example configs. Accepted as a false positive, not a hole. |

Looked at and left as they are:

- `AuditRecord` canonicalises the config path, a filesystem read outside `Config::load`. It is a read with a fallback, and moving it would only spread the path handling around.
- `Decision::as_str` and `AuditLevel::as_str` repeat their serde names. Plain `match`es are clearer than going through serde for a string.
- `scripts/copilot-verify.sh` is fine for a hand-run kit: `set -euo pipefail`, quoted expansions except the deliberate `$flags`, and a dry-run `push`. No changes.
- The repository and its directory are still named `claude-code-permissions-hook`; renaming them is a post-merge step for the author.
