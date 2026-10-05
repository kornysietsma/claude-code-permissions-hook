# Plan: shell-aware rules

Implements [`spec.md`](./spec.md) in vertical slices. Each step leaves the tool working, passes the quality gate, and can be checked by hand. Tick the boxes as steps land; record surprises in [Findings](#findings).

Quality gate, for every step:

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

## Technical context

### Where things go

| Module | Change |
|--------|--------|
| `src/shell.rs` (new; split into `src/shell/{mod,walk,words}.rs` if it passes ~400 lines) | Pure: `analyse(command, cwd, home, settings) -> Analysis`. Parsing, the walker, word unquoting, segments, constructs and floors, wrappers, `cd` tracking, path-like detection. No policy knowledge. |
| `src/agent.rs` | `Agent::shell_tool() -> ShellTool { name, command_path }`, and `Agent::shell_payload(command, cwd) -> Value` for `explain`. |
| `src/config.rs` | `[[command_rule]]`, `[[construct_rule]]`, `[shell]`; the new config errors. |
| `src/policy.rs` | Rule kinds, command and construct rule evaluation, `paths_under`, the combiner. |
| `src/auditing.rs` | `kind` on rule references, the `shell` object, truncation applied to it. |
| `src/lib.rs` | `run()` unchanged in shape; a shared inner function also used by `explain()`. |
| `src/main.rs` | `explain` subcommand. |
| `src/validate.rs` | Summary and checks for the new rule kinds. |

### Key types (sketch; adjust as the code asks)

```rust
// shell.rs
pub struct Analysis { pub segments: Vec<Segment>, pub constructs: Vec<Construct> }

#[derive(Serialize)]
pub struct Segment {
    pub text: String, pub name: String, pub args: Vec<String>,
    pub env: BTreeMap<String, String>, pub redirects: Vec<Redirect>,
    pub wrappers: Vec<String>, pub source: String,
    #[serde(skip)] pub kind: SegmentKind,      // Command | Cd | AssignmentOnly
    #[serde(skip)] pub cwd: PathBuf,           // effective directory, after earlier `cd`s
}

pub struct Construct { pub kind: ConstructKind, pub segment: Option<usize>, pub detail: Option<String> }
pub enum ConstructKind { // configurable
    RedirectWrite { target: PathBuf }, RedirectRead, Heredoc, Pipe, Background, Subshell, Substitution,
    // floors
    ParseError, Unsupported, ShellReentry, ExecTool, DynamicCommand, Expansion, EnvAssign, Cd,
}
impl ConstructKind { pub fn is_floor(&self) -> bool; pub fn name(&self) -> &'static str; pub fn describe(&self) -> &'static str }

// policy.rs
pub enum RuleKind { Rule, CommandRule, ConstructRule, Floor }
pub struct Match { kind, index, decision, description, reason, segment: Option<usize> }
pub struct Evaluation { matches: Vec<Match>, shell: Option<ShellEvaluation> }
pub struct ShellEvaluation { analysis: Analysis, segment_decisions: Vec<SegmentDecision> }
```

`Evaluation` stops borrowing `&Rule` and owns plain `Match` values; that's simpler once floors (which have no rule) are matches too.

### Design decisions

- **Two-level parsing with brush-parser.** `brush_parser::Parser::parse_program` gives the AST, where words are raw strings (`ast::Word { value, loc }`); `brush_parser::word::parse` splits a word into `WordPiece`s (`Text`, `SingleQuotedText`, `AnsiCQuotedText`, `DoubleQuotedSequence`, `TildeExpansion`, `ParameterExpansion`, `CommandSubstitution(String)`, `BackquotedCommandSubstitution(String)`, `EscapeSequence`, `ArithmeticExpression`). A word is **static** if it has only literal pieces (and a leading `Home` tilde); the static value is built from the pieces. Any other piece means `expansion`. Unquoted `Text` is also checked for glob characters (`brush_parser::pattern::pattern_has_glob_metacharacters`), brace expansion (`word::parse_brace_expansions`) and a leading `=`.
- **Parser options**: bash mode, not POSIX or sh mode. Extended globbing is **on**, so `@(…)` and `*(…)` parse as globs (→ `expansion`) rather than errors. Decide in the spike if that misbehaves.
- **Allow-listed walker.** The walker matches on the AST enums exhaustively. Supported: `Program`, `CompoundList`, `AndOrList`, `Pipeline` (including `time`), `SimpleCommand`, `SubshellCommand`, `BraceGroupCommand`, redirects and heredocs. Everything else (`if`, `for`, `while`, `case`, arithmetic commands, `[[ … ]]`, functions, coprocesses) produces `Unsupported`. Exhaustive `match` with no `_ =>` arm on brush's enums means a brush upgrade that adds a variant fails to compile, rather than silently allowing.
- **Recursion.** `CommandSubstitution(String)` and backquoted substitutions are re-parsed with `parse_program` and walked. Process substitutions arrive as AST `SubshellCommand`s. Recursion depth is capped (16); deeper means `Unsupported`.
- **Segments are matched as JSON.** `serde_json::to_value(&segment)` is fed to the existing `FieldCondition` code, so command rules reuse every matcher. `FieldCondition::matches` changes from taking a `ToolCall` to taking `(value, cwd, home)`, so `under` on segment fields resolves against the segment's effective directory.
- **Rule indexes are per kind**, because TOML deserialises each array of tables separately and loses their relative order.
- **The combiner** works on a flat list of `Match`es plus per-segment decisions, in the spec's order: `[[rule]]` matches (file order), floors (textual order), segments (textual order), construct rules (file order).
- **Tests build Bash payloads in code.** A `common::claude_bash(command, cwd)` helper (and a Copilot equivalent) avoids a fixture per command. The captured fixtures stay for payload-shape tests.
- **Real-world corpus.** `scripts/shell-corpus.sh` extracts the unique shell commands from an audit log (with `jq`) into `tests/fixtures/shell/corpus.local.txt`, which is **gitignored**: it may contain private paths. An `#[ignore]`d test analyses every line and prints a summary (counts per floor, parse errors), for spotting gaps. A sanitised, curated subset is committed as `tests/fixtures/shell/corpus.txt` and asserted in normal tests.
- **Differential tests** live in `tests/shell_differential.rs`. For each case, run `<shell> -c '__d() { printf "%s\0" "$@"; }; __d <command line>'` with `zsh -f` and `/bin/bash`, and compare the NUL-separated words with our `[name] + args`. Skip a shell that isn't installed. Only static simple commands are used; cases where the shells disagree are asserted to hit a floor instead.

### Things to keep in mind throughout

- `run` must always exit 0. Nothing in the shell analysis may panic: parse failures become constructs, not errors. Use no `unwrap` on parser output.
- Keep the payload a `serde_json::Value`.
- Don't install this branch's binary until step 11. After step 2, the author's live config is invalid under the new binary (every call would get `ask`).
- The live hook is still the old binary while we work. Write commit messages with a file tool and `git commit -F`, never heredocs that mention Python tools.

## Steps

### Step 1: spike brush-parser

- [ ] Add `brush-parser = "=0.4.0"` (pinned). Check `cargo tree` for the dependency footprint, and that the build still passes the gate.
- [ ] Write `scripts/shell-corpus.sh` and gitignore `tests/fixtures/shell/corpus.local.txt`.
- [ ] A temporary `#[ignore]`d test that, for each corpus line and a list of nasty cases, prints the AST and the word pieces for every word.
- [ ] Confirm, and record in Findings:
  - quoted versus unquoted pieces are distinguishable, and `$'…'` and escape sequences are decoded, or we can decode them ourselves;
  - heredoc bodies and whether the delimiter was quoted are available;
  - redirect fds and operators are available;
  - source spans are good enough for `source` (or fall back to a re-rendered string);
  - parse errors are returned, not panics, for garbage input;
  - extglob on versus off for `*(…)`;
  - how `time`, `!`, `coproc` and `[[ ]]` appear.
- [ ] Decide go or no-go. If no-go, switch to `tree-sitter-bash` and redo this step.

**Verify:** the gate passes. The Findings section answers each bullet above. The spike test is either deleted or turned into the start of step 2's unit tests.

### Step 2: first end-to-end slice (compound commands, command rules)

The smallest useful thing: `cargo build 2>&1 && cargo test` is allowed by command rules.

- [ ] Refactor, with no behaviour change, first and committed separately:
  - `Evaluation` owns `Match` values with a `RuleKind`;
  - `FieldCondition::matches(value, cwd, home)`;
  - audit references gain `"kind": "rule"`. Update the audit tests.
- [ ] `Agent::shell_tool()`.
- [ ] `shell::analyse` for lists, pipelines, subshells, brace groups and simple commands with static words; redirects recorded. The `ParseError` and `Unsupported` floors. Any non-static word is `Unsupported` for now; step 4 refines it to `Expansion` and `DynamicCommand`.
- [ ] `text` re-quoting: plain if `[A-Za-z0-9_@%+=:,./-]+`, else single-quoted with `'\''`.
- [ ] Config: `[[command_rule]]` (decision, description, reason, match), with an error for a command rule with no conditions. An error for an allow `[[rule]]` that can match the shell tool (`tool` omitted, or its regex matches the shell tool name).
- [ ] Policy: for shell-tool calls, run the analysis, evaluate command rules per segment, and combine. Floors become `Match`es of kind `Floor`. The reason gets ` — in "<text>"` or ` — <construct description> in "<source>"`.
- [ ] Missing or non-string command field → `Unsupported`.
- [ ] Migrate the tests that use Bash allow `[[rule]]`s (`tests/claude.rs`, `tests/copilot.rs`, `tests/smoke.rs`, `tests/examples.rs`) to command rules.
- [ ] Migrate `examples/*.toml`: shell allows become command rules on `text`. Remove `@shell_chain`, keep `@parent_dir` where still needed until step 7. `legacy_python` becomes a deny command rule on `name`. Add the harmless-builtins allow and the `sudo` ask. Run `validate` on each.
- [ ] Differential tests (`tests/shell_differential.rs`) for static simple commands.

**Automated:** unit tests for re-quoting and static word values. Acceptance tests:
- `cargo build 2>&1 && cargo test` → allow;
- `foo && cargo test` → passthrough;
- `cargo test; rm -rf /` with a deny → deny;
- `if true; then cargo test; fi` → ask (`unsupported`);
- unbalanced quotes → ask (`parse_error`);
- a heredoc commit message with a quoted delimiter and a `pip install` line → allowed by a `git commit` rule, not denied;
- the allow-`[[rule]]`-on-Bash config error, with and without `tool`;
- the same for Copilot `bash` and `copilot_via_claude`.

**Manual:** `cat tests/fixtures/claude/bash.json | cargo run -- run --agent claude --config examples/claude.toml`, plus a hand-made payload for a compound command.

### Step 3: audit `shell` object and `explain`

Built early so every later step can be checked by hand.

- [ ] Audit: the `shell` object (segments with their own matches and decision; constructs with `floor`). `kind` on all references. Truncation applied to string values in `shell`. Shell calls that hit a floor count as "matched".
- [ ] `lib::explain(agent, config_path, input, context) -> Result<Value>`: the same evaluation as `run`, with the record built as if `level = "all"` and `max_value_len = 0`. Config errors are returned as `Err`.
- [ ] CLI: `explain --agent A [--config C] [--cwd D] (COMMAND | --payload FILE)`. `--payload` accepts a raw payload or an audit record (take its `payload`). Pretty-print the JSON. Exit 1 on errors.
- [ ] Smoke test: spawn `explain` on a compound command and on an audit record file; parse the JSON; check the decision and segment count.

**Automated:** audit tests for the `shell` shape (full `assert_eq!` on one representative record), truncation inside `shell`, and `explain` with both input forms.

**Manual:** `cargo run -- explain --agent claude --config examples/claude.toml 'cargo build 2>&1 && cargo test | tee out.txt'`. Read the JSON and confirm it's understandable without the docs.

### Step 4: substitutions, expansions, dynamic command names

- [ ] Word classification: static, or expansion with a detail (which piece). Unquoted globs, brace expansion, and a leading `=` count as expansion.
- [ ] Recursion into `$( )`, backticks and process substitutions, with the depth cap. Inner segments join the segment list.
- [ ] Heredocs: an unquoted delimiter means the body is parsed with `word::parse_heredoc`, any expansion → `Expansion`, and substitutions are recursed. A quoted delimiter means a literal body. Here-strings are treated like words.
- [ ] Floors: `Expansion` (args, env values, redirect targets, heredoc bodies) and `DynamicCommand` (command word). These replace step 2's temporary `Unsupported`.
- [ ] Constructs recorded (not yet configurable): `Substitution`, `Heredoc`, `Pipe`, `Background`, `Subshell`, `RedirectRead`, `RedirectWrite`.

**Automated:**
- `echo $(curl x)` with a deny on curl → deny; without the deny → ask;
- `ls *.rs` → ask; `ls '*.rs'` → allowed by an `ls` rule;
- `echo {a,b}` → ask; `$CMD x` → ask (`dynamic_command`); `=python3 x` → ask;
- `cat <<EOF` with `$(curl x)` → ask, and deny with a curl deny rule; `cat <<'EOF'` with the same body → allowed by a `cat` rule;
- `echo "$HOME"` → ask; `echo '$HOME'` → allowed by an `echo` rule;
- a zsh/bash disagreement table in the differential tests, asserting a floor.

**Manual:** `explain` a few real messy commands from the corpus; run the ignored corpus test and skim the floor counts.

### Step 5: wrappers, `[shell] safe_env`, `env_assign`

- [ ] `[shell]` config with `safe_env` (regexes and `@patterns`); unknown keys are errors.
- [ ] Inline assignment prefixes go to `env`. Assignment-only segments (neutral, `SegmentKind::AssignmentOnly`). `export`, `declare`, `typeset`, `local` and `readonly` `NAME=value` args are checked too.
- [ ] The `EnvAssign` floor for names not matching `safe_env`.
- [ ] Wrapper unwrapping (`env`, `timeout`, `nice`, `nohup`, `time`) with the option grammar from the spec; unknown options → `Unsupported`. `wrappers` is recorded.

**Automated:**
- `RUST_LOG=debug cargo test` and `env RUST_LOG=debug cargo test` → allow with `safe_env`;
- `GIT_PAGER=x git log`, `PATH=./evil cargo test`, `PATH=./evil; cargo test` and `export GIT_PAGER=x; git log` → ask;
- `timeout 60 cargo test`, `timeout -s KILL 60 cargo test` and `nice -n 5 cargo test` → allow;
- `timeout --bogus 60 cargo test` and `env -i cargo test` → ask;
- bare `timeout` → a segment named `timeout`;
- unit tests for each wrapper's option parsing.

**Manual:** `explain 'env RUST_LOG=debug timeout 60 cargo test'` and check the `env` and `wrappers` fields.

### Step 6: `shell_reentry` and `exec_tool` floors

- [ ] `ShellReentry` by `name` basename, using the spec's list.
- [ ] `ExecTool`: `xargs`, and `find` with `-exec`, `-execdir`, `-ok`, `-okdir` or `-delete`.

**Automated:**
- `bash -c 'ls'`, `sh x.sh`, `curl x | sh`, `eval x`, `source x`, `. x`, `alias ls=x`, `setopt x` and `exec cargo test` → ask, even with a broad allow rule;
- a deny still wins over the floor;
- `find . -name '*.rs'` → allow with a `find` rule; `find . -exec rm {} \;` and `find . -delete` → ask;
- `ls | xargs rm` → ask;
- `/bin/bash x` → ask (basename).

**Manual:** `explain` on a pipe into a shell; check that the reason text reads well.

### Step 7: `cd` tracking and `paths_under`

- [ ] `cd` / `pushd` to a static path inside `{cwd}` is neutral (`SegmentKind::Cd`) and updates the effective directory for later segments. Anything else is the `Cd` floor (bare `cd`, `cd -`, `popd`, options, outside paths, non-static paths).
- [ ] `paths_under` on command rules: path-like detection (args with `/`, or starting with `.` or `~`; the value part of `--opt=value`; redirect targets except fd dups and `/dev/null`). Resolve against the segment's effective directory with `paths::resolve`.
- [ ] `~` expansion in static words (leading `~` or `~/`). `~user`, `~+` and `~-` are `Expansion`.
- [ ] A `cd`-only command → passthrough.
- [ ] Examples: replace the remaining `@parent_dir` uses with `paths_under`. Remove the `parent_dir` pattern.

**Automated:**
- `rm build/x` with `paths_under = ["{cwd}"]` → allow; `rm /etc/x` and `rm ../x` → passthrough;
- `rm --out=/etc/x` → passthrough; `cp a b > /etc/x` → passthrough;
- `cmd 2>&1 > /dev/null` → `paths_under` unaffected;
- `cd sub && rm x` → allow; `cd sub && rm ../../x` → passthrough; `cd /tmp && rm x` → ask; `cd && ls` → ask;
- a symlink inside the project pointing outside → passthrough (temp dir test);
- `rm ~/x` with `paths_under = ["~/scratch"]` → passthrough, `rm ~/scratch/x` → allow (with an injected home).

**Manual:** `explain 'cd src && rm -f old.rs'` with the example config and check each segment's resolution.

### Step 8: construct rules

- [ ] Config: `[[construct_rule]]` with `decision`, `construct`, `description`, `reason`, and `outside` (only for `redirect_write`). Errors: allow, an unknown construct, a floor name, `outside` on another construct.
- [ ] Evaluation: a construct rule matches if any recorded construct of its kind is present (for `redirect_write` with `outside`, only targets not under those directories, resolved against the segment's effective directory).
- [ ] Examples: ask on `background`; ask on `redirect_write` outside `{cwd}` and `/tmp`.

**Automated:**
- `cargo test > /tmp/out.txt` → allow; `cargo test > ~/out.txt` → ask; `cargo test > out.txt` → allow;
- `cd sub && cargo test > ../../x` → ask;
- `cargo test &` → ask;
- each config error.

**Manual:** `explain` a command that triggers both a command rule allow and a construct rule ask; check which one is the deciding item and that the reason makes sense.

### Step 9: `validate` and reason polish

- [ ] The summary: counts per rule kind and decision, construct rules by construct, `safe_env`, patterns, audit.
- [ ] Warnings: command rule match paths whose first component isn't a segment field. The existing payload-key warning stays for `[[rule]]`.
- [ ] Review every floor's `describe()` text and the reason formats against real examples; keep them short.

**Automated:** validate summary snapshot tests (`assert_eq!` on the full text) for the example configs; the warning tests.

**Manual:** run `validate` on each example and read the output.

### Step 10: docs, README, AGENTS.md and skills

- [ ] `docs/configuration-guide.md`: a new shell section (segments, command rules, `paths_under`, construct rules, floors, `[shell]`, evaluation order, `explain`, zsh notes). Update the regex caveats, the shell-chaining note, the worked examples and the audit section. Update rule indexes to per-kind.
- [ ] `README.md`: a short pitch for compound commands, and `explain`.
- [ ] `AGENTS.md`: the code structure (`shell.rs`), the out-of-scope list (compound commands are now parsed; control structures aren't yet), the brush-parser pinning note, and removal of the `legacy_python` heredoc caveat (keep it until step 11 is done).
- [ ] `examples/skills/tool-gate-rules-claude/SKILL.md` and `…-copilot/SKILL.md`, following the spec's outline, with valid YAML frontmatter (`name`, `description`).
- [ ] Test: every TOML snippet in the skills under a "recipe" marker is extracted and loaded with `Config::from_toml` for its agent (`tests/examples.rs`).

**Automated:** the skill-recipe test; the examples test (count updated).

**Manual:**
- Read the guide top to bottom as a new user.
- **Try a skill with a cheap model.** In a scratch project, install the Claude skill. Ask Haiku to (a) allow `npm run lint` and (b) turn a real audit record into a rule. Check the rules are narrow and the model used `explain` to confirm them.

### Step 11: migrate the live config and dogfood

- [ ] Rewrite `~/.config/tool-gate-hook/claude.toml` (and Copilot's) in the new format; `validate`.
- [ ] Replay the corpus: for each line in `corpus.local.txt`, compare the old decision (from the audit log) with the new `explain` decision. Review every change of decision, especially new allows.
- [ ] `cargo install --path .`, then use Claude Code for a session with `level = "all"`. Review the audit log for surprising floors or allows.
- [ ] Copilot: run the relevant parts of `docs/copilot-verification.md` with a compound command; fold the results back into fixtures.
- [ ] Remove the `legacy_python` heredoc caveat from AGENTS.md.
- [ ] Fold the remaining spec content into the docs and AGENTS.md, and remove `spec.md` and `plan.md` (as was done after the last rework).

**Verify:** the replay diff is reviewed with no unexplained new allows, and a day of normal use passes without false denies.

## Verification approach (overall)

- **Security-relevant behaviour is tested as acceptance tests** through `tool_gate_hook::run`. Each floor has at least one test showing it overrides a broad allow, and one showing a deny still beats it.
- **Parser agreement** is covered by the zsh/bash differential tests. Shell disagreements are explicit tests that land on floors.
- **Real-world fit** comes from the local corpus (an ignored test, plus the step 11 replay) and the committed curated corpus.
- **Readability** is checked by hand with `explain` at each step.
- Coverage follows AGENTS.md: main behaviours and security edges. No concurrency or process orchestration beyond the existing smoke tests and the differential tests.

## Findings

Record spike results and surprises here as steps land.
