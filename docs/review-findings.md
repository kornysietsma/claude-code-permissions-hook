# Review findings (pre-rework)

This is a light review of `claude-code-permissions-hook` at commit `acf5d88`, done before the `tool-gate-hook` rework. Its purpose is to carry lessons into the rework rather than fix code that is about to be replaced. Items marked **confirmed** were reproduced by running the binary.

## Behaviour bugs

| # | Finding | Status | Addressed by |
|---|---|---|---|
| 1 | **Typos in config keys are silently ignored.** `comand_regex = "rm"` in a deny rule passes `validate` and never matches, so a mistyped deny rule silently disappears. | confirmed | `deny_unknown_fields` (step 2.1) |
| 2 | **A rule with no main regex never matches.** `[[deny]] tool = "Bash"` with no `command_regex` does nothing. An exclude regex without a main regex is ignored too. | by reading | Rules with no `match` entries match on `tool` alone (2.1/2.2) |
| 3 | **Fields that don't apply to the rule's tool are silently ignored.** `command_regex` on a `Read` rule is accepted and never evaluated. | by reading | Field paths address the payload directly, and `validate` warns on unknown roots (6.1) |
| 4 | **`Task` rules OR their conditions.** A rule with both `subagent_type` and `prompt_regex` matches if *either* matches, which is surprising for a deny or allow. | by reading | All `match` entries are ANDed (2.2) |
| 5 | **A payload missing any expected field fails the hook with exit 1.** `HookInput` requires `session_id`, `transcript_path` and `hook_event_name`. Claude treats exit 1 as a non-blocking error, so the call fails open with only a stderr message. Under Copilot, exit 1 would mean deny. | confirmed | Payload kept as raw `Value`, mismatch passes through, always exit 0 (2.2/2.3) |
| 6 | **Config errors fail open silently.** A missing or broken config makes `run` exit 1, so Claude proceeds through its normal flow and nothing visible tells you the hook is off. | by reading | Config error produces `ask` (2.3) |
| 7 | **`validate` prints nothing.** Its summary is logged at `info!`, but the default log filter is `warn`, so a valid config produces no output at all. | confirmed | Print the summary to stdout (6.1) |
| 8 | **Only hardcoded tools can be ruled on.** Tools are limited to `Read`/`Write`/`Edit`/`Glob`/`Bash`/`Task`. Rules for any other tool (Grep, WebFetch, MCP tools, `Agent`) are silently ignored, and tool matching is exact-string only. | by reading | `tool` regex plus field paths (2.1/2.2) |

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

## Fixed in phase 0

- Removed unused `lazy_static`, `derive_builder` and `itertools`. Replaced `nix` flock with std `File::lock`. All dependencies are at their latest stable versions (step 0.1).

Nothing else was fixed: every finding above is in code that phases 1–5 replace.
