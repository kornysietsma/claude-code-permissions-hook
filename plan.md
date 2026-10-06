# Plan: shell-aware rules

Implements [`spec.md`](./spec.md) in vertical slices. Each step leaves the tool working, passes the quality gate, and can be checked by hand. Tick the boxes as steps land, and record anything later steps need in [Notes for later steps](#notes-for-later-steps).

Quality gate, for every step:

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

## Status

- **Branch** `shell-parsing`, not pushed; the PR is raised at the end. The last commit is `af3a07f` (step 4).
- **Done:** steps 1 to 4.
- **Next:** step 5, wrappers, `[shell] safe_env` and `env_assign`.

### Working on this branch

- **Don't `cargo install` this branch until step 11.** The author's live config (`~/.config/tool-gate-hook/claude.toml`) has Bash allow `[[rule]]`s, which are now a config error, so the new binary would `ask` on every call.
- **The live hook is still the old binary.** Its `legacy_python` rule denies any Bash command whose text contains a Python tool name where a command could start, including inside heredocs, `$( )` and `<( )`. So:
  - write files containing such text (example configs, tests, payload files) with the Write/Edit tools, not `cat <<EOF`, perl or sed;
  - to `explain` such a command, put a payload in a scratchpad file and use `--payload`;
  - write commit messages to a file in the scratchpad and use `git commit -F`.
- **Something between the agent and the file turns a backslash-u unicode escape in written source into the literal character.** Build such strings at runtime, e.g. `format!("$'{}u00e9'", '\\')`.
- **Claude Code's built-in `rm` safety check blocks inline `zsh -c '…'` / `bash -c '…'` scripts run from the Bash tool**, even without `rm`. Write a script file to the scratchpad and run `zsh -f FILE`, or compare from Rust tests (`tests/shell_differential.rs`).
- **The Bash tool runs zsh with `EQUALS` on**, so `echo ====` fails there ("=== not found") and skips the rest of the line. Use `echo ---` as a separator.
- **Local corpus** (gitignored, may contain private paths):
  - build it with `scripts/shell-corpus.sh ~/.local/share/tool-gate-hook/claude.jsonl`, which writes `tests/fixtures/shell/corpus.local.jsonl` (one JSON string per line);
  - summarise it with `cargo test --test shell_corpus -- --ignored --nocapture`, which lists each floor's details per command;
  - after step 4, 122 of 162 commands hit no floor. Most floors are `$VAR` from `S=…` assignments, and globs.
- **`explain`** is the quickest manual check: `cargo run -q -- explain --agent claude --config examples/claude.toml --cwd /tmp 'COMMAND' | jq …`. An empty config file shows the bare analysis.

## Technical context

### Code shape (as of step 4)

| Module | Contents | Still to come |
|--------|----------|---------------|
| `src/shell/mod.rs` | `analyse(command, home) -> Analysis`; the allow-listed `Walker` (with `depth`, capped at `MAX_DEPTH` = 16); `Segment`, `Redirect`, `Construct { kind, segment, detail, target }`, `ConstructKind` (floors `ParseError`, `Unsupported`, `Expansion`, `DynamicCommand`; recorded `Substitution`, `Heredoc`, `Pipe`, `Background`, `Subshell`, `RedirectRead`, `RedirectWrite`; `name()`, `is_floor()`, `describe()`); `Analysis::unsupported(detail)`. Words and redirects queue `Pending` substitutions, which `walk_pending` walks after the segment is pushed. `enclosing()` records pipe, background and subshell against the first segment inside. A word's `Role` (command, arg, redirect, here-string) picks the floor and the detail wording. `file_redirect()` records `redirect_read`/`redirect_write`. | Floors `EnvAssign` (5), `ShellReentry`, `ExecTool` (6), `Cd` (7); wrappers and env (5); `cd` and the effective directory (7). `analyse` will need `cwd` and `[shell]` settings. |
| `src/shell/words.rs` | `analyse(raw, home, options) -> WordInfo { value: Result<String, NotStatic>, substitutions }`, `analyse_heredoc(raw, options)`; `NotStatic::{Expansion, Unsupported}(why)`; `$'…'` decoding; `quote()` for `text`. Unit tests. | |
| `src/agent.rs` | `Agent::shell_tool() -> ShellTool { name, command_path }`; `Agent::shell_payload(command, cwd)`, a minimal payload used by `explain` and tests. | |
| `src/config.rs` | `Config::load(path, agent)` / `from_toml(contents, agent)`; `[[command_rule]]`; the errors for an allow `[[rule]]` that can match the shell tool, and for a command rule with no conditions. | `[shell]` (5), `paths_under` (7), `[[construct_rule]]` (8). |
| `src/policy.rs` | `Policy { rules, command_rules, shell_tool }`; `RuleKind` (`Rule`, `CommandRule`, `Floor`); `Rule` (reused for command rules, `tool: None`); `Match { kind, index: Option, decision, description, reason, segment: Option (1-based) }`; `Evaluation { matches, decided_by, shell: Option<ShellEvaluation { analysis, segment_decisions: Vec<Option<Decision>> }> }`; the combiner in `Policy::evaluate`; `floor_match`. | Neutral segments (5, 7), `paths_under` (7), `ConstructRule` kind and evaluation (8). |
| `src/auditing.rs` | `AuditRecord::for_evaluation(invocation, payload, evaluation, max_value_len)` builds the record, including the `shell` object (`ShellRecord`, serialised to a `Value` and truncated); the caller checks the level with `AuditConfig::records`. `AuditRecord::for_explanation(…, truncated)` adds `reason`. `is_truncated`, `truncate_json_strings`. | `"decision": "neutral"` for neutral segments (5, 7). |
| `src/validate.rs` | Summary lines for rules and command rules. | Construct rules, `safe_env`, segment-field warnings (9). |
| `src/lib.rs`, `src/main.rs` | `run`, `validate`, and `explain` (`lib::explain(agent, config_path, input, context) -> Result<Explanation { record, warnings }>`; the input is a payload or an audit record; `--payload -` reads stdin). | |

Tests:

| File | Contents |
|------|----------|
| `tests/shell.rs` | Acceptance tests through `run` for both agents, with one shared `CONFIG` (rule numbers are referenced in reason assertions, so append new rules at the end). Its `shell_payload(agent, command)` helper wraps `Agent::shell_payload` with cwd `/tmp`. Put helpers here rather than in `tests/common/mod.rs`, where an unused function is a dead-code error in other test binaries. |
| `tests/shell_differential.rs` | `AGREED` (static commands zsh, bash and we split identically) and `DISAGREED` (must hit a floor). |
| `tests/shell_corpus.rs` | `curated_corpus_is_analysed_as_expected` checks `tests/fixtures/shell/corpus.jsonl`: sanitised real commands with expected segment names and floors. To add cases, generate the expectations with `explain`, an empty config and `jq`, then review them. Plus the ignored local-corpus summary. |
| `tests/audit.rs` | Audit records, including the full `shell` shape (`shell_record_shows_each_segment_and_construct`) and truncation inside `shell`. |
| `tests/explain.rs` | `lib::explain`: both input forms, both agents, reasons, truncated records, errors. `tests/smoke.rs` spawns the `explain` CLI. |
| `tests/examples.rs` | The example configs, including the legacy-python cases (denied inside `$( )`, backticks, `<( )` and unquoted heredocs; not denied in quoted text). |

### Design decisions

- **Two-level parsing.** `brush_parser::Parser::new(Cursor::new(cmd), &ParserOptions::default()).parse_program()` gives the AST, where words are raw strings (`ast::Word { value, loc }`). `brush_parser::word::parse(raw, &options)` splits a word into `WordPiece`s. Default options are bash mode, with extended globbing on: `@(…)` parses as text, which the glob check catches.
- **Allow-listed walker with exhaustive matches** on brush's enums (no `_ =>` arms), so a brush upgrade that adds syntax fails to compile instead of being silently allowed. brush-parser is pinned to `=0.4.0`.
- **Segment order**: textual, except that a command comes before the commands substituted into it (substitutions are queued as `Pending` and walked after the segment is pushed, so segment indexes stay stable).
- **Segments are matched as JSON**: `serde_json::to_value(&segment)` goes through the existing `FieldCondition` code. `FieldCondition::matches(value, cwd, context)` takes a cwd so that, from step 7, `under` can use a segment's effective directory.
- **Combiner** (`Policy::evaluate`): deny or ask if any match has it (the first most restrictive match decides); else allow if there is at least one segment and every segment's decision is allow (the first allow match decides); else passthrough. When steps 5 and 7 add neutral segments (assignment-only, `cd`), they must be skipped in the "every segment" check, and a command with only neutral segments passes through.
- **Rule indexes are per kind**, because TOML deserialises each array of tables separately and loses their relative order.
- **Floor details** are short and specific (`for loop`, `variable in $HOME`, `assignment FOO`), and they appear in reasons.
- **serde_json has `preserve_order`**, so records and `explain` output keep struct order.

### brush-parser facts needed for later steps

- **Assignments**: `CommandPrefixOrSuffixItem::AssignmentWord(Assignment { name: AssignmentName, value: Scalar(Word) | Array(..), append, .. }, Word)`, in the prefix for `FOO=1 cmd`, and in the suffix for declaration builtins (`export X=1`). An assignment-only command is a `SimpleCommand` with `word_or_name: None`. Treat array values as unsupported.
- **`time`** is `Pipeline.timed`; **`!`** is `Pipeline.bang`. Pipeline items are `(PipelineOperator, &Pipeline)` from `AndOrList::iter()`. `PipelineOperator` has no `Debug`.
- **Redirects** carry no source location; **source spans** are character indexes, so use the `Display` rendering of `SimpleCommand` for `source`.
- **Parse errors** come back as `Err` (never seen to panic).
- A quoted heredoc delimiter arrives raw (`'EOF'`); the walker unquotes it.

### Things to keep in mind throughout

- `run` must always exit 0. Nothing in the shell analysis may panic, so no `unwrap` on parser output.
- Keep the payload a `serde_json::Value`.
- Every floor needs a test showing it beats a broad allow, and one showing a deny still beats the floor.

## Steps

### Steps 1–4 ✅

1. **brush-parser spike.** brush-parser 0.4.0 is in.
2. **First end-to-end slice.** Segments; command rules on `text`/`name`; the `parse_error` and `unsupported` floors; an allow `[[rule]]` on the shell tool is a config error; the examples migrated; differential tests against zsh and bash.
3. **Audit `shell` object and `explain`.** Including the `reason` in `explain` output, and `ask` for truncated audit records.
4. **Substitutions, expansions, dynamic command names.** Word classification; the `expansion` and `dynamic_command` floors; recursion into substitutions (depth 16); unquoted heredoc bodies; non-floor constructs recorded; the curated corpus; `====`-style words are static.

### Step 5: wrappers, `[shell] safe_env`, `env_assign`

- [ ] `[shell]` config with `safe_env` (regexes and `@patterns`); unknown keys are errors.
- [ ] Inline assignment prefixes go to `env`. Assignment-only segments are neutral (`SegmentKind::AssignmentOnly`, skipped by the combiner's "every segment" check). The `NAME=value` args of `export`, `declare`, `typeset`, `local` and `readonly` are checked too (step 2 treats them as plain args).
- [ ] The `EnvAssign` floor for names not matching `safe_env`. This replaces step 2's temporary `Unsupported` for prefix assignments and assignment-only commands.
- [ ] Wrapper unwrapping (`env`, `timeout`, `nice`, `nohup`; `time` is already recorded) with the spec's option grammar; unknown options are `Unsupported`. `wrappers` is recorded.
- [ ] Example configs: add a `[shell] safe_env` list.

**Automated:**
- `RUST_LOG=debug cargo test` and `env RUST_LOG=debug cargo test` → allow with `safe_env`;
- `GIT_PAGER=x git log`, `PATH=./evil cargo test`, `PATH=./evil; cargo test` and `export GIT_PAGER=x; git log` → ask;
- `FOO=1` alone → passthrough (only neutral segments);
- `RUST_LOG=$x cargo test` → ask (`expansion` in the env value), and `X=$(curl x)` → deny with a curl deny rule;
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

- [ ] `analyse` takes the payload cwd. `cd` / `pushd` to a static path inside `{cwd}` is neutral (`SegmentKind::Cd`) and updates the effective directory for later segments (`Segment.cwd`, `#[serde(skip)]`). Anything else is the `Cd` floor (bare `cd`, `cd -`, `popd`, options, outside paths, non-static paths).
- [ ] `paths_under` on command rules: path-like detection (args with `/`, or starting with `.` or `~`; the value part of `--opt=value`; redirect targets except fd dups and `/dev/null`). Resolve against the segment's effective directory with `paths::resolve`. Pass `Segment.cwd` into `matches_value` too, so `under` on segment fields agrees.
- [ ] A `cd`-only command → passthrough.
- [ ] Examples: replace the remaining `@parent_dir` uses (mermaid) with `paths_under`, and remove the `parent_dir` pattern.

**Automated:**
- `rm build/x` with `paths_under = ["{cwd}"]` → allow; `rm /etc/x` and `rm ../x` → passthrough;
- `rm --out=/etc/x` → passthrough; `cp a b > /etc/x` → passthrough;
- `cmd 2>&1 > /dev/null` → `paths_under` unaffected;
- `cd sub && rm x` → allow; `cd sub && rm ../../x` → passthrough; `cd /tmp && rm x` → ask; `cd && ls` → ask;
- a symlink inside the project pointing outside → passthrough (temp dir test);
- `rm ~/x` with `paths_under = ["~/scratch"]` → passthrough, `rm ~/scratch/x` → allow (with an injected home).

**Manual:** `explain 'cd src && rm -f old.rs'` with the example config and check each segment's resolution.

### Step 8: construct rules

- [ ] Config: `[[construct_rule]]` with `decision`, `construct`, `description`, `reason`, and `outside` (only for `redirect_write`). Errors: allow, an unknown construct, a floor name, `outside` on another construct. Add `RuleKind::ConstructRule`.
- [ ] Evaluation: a construct rule matches if any recorded construct of its kind is present. For `redirect_write` with `outside`, only targets not under those directories count, resolved against the segment's effective directory (the cwd for a group's own redirects, which have no segment). Matches go after the command-rule matches.
- [ ] Examples: ask on `background`; ask on `redirect_write` outside `{cwd}` and `/tmp`. Add the harmless-builtins allow (`true`, `false`, `:`, `echo`, `printf`, `test`, `[`), held back from step 2 until writes are checked.

**Automated:**
- `cargo test > /tmp/out.txt` → allow; `cargo test > ~/out.txt` → ask; `cargo test > out.txt` → allow;
- `echo x > ~/.zshrc` → ask with the example config;
- `cd sub && cargo test > ../../x` → ask;
- `cargo test &` → ask;
- each config error.

**Manual:** `explain` a command that triggers both a command rule allow and a construct rule ask; check which is the deciding item and that the reason makes sense.

### Step 9: `validate` and reason polish

- [ ] The summary: counts per rule kind and decision (rules and command rules done in step 2), construct rules by construct, `safe_env`, patterns, audit.
- [ ] Warnings: command rule match paths whose first component isn't a segment field. The existing payload-key warning stays for `[[rule]]`.
- [ ] Review every floor's `describe()` text and the reason formats against real examples; keep them short. Consider whether `source` should undo brush's `2>& 1` rendering.

**Automated:** validate summary tests (`assert_eq!` on the full text) for the example configs; the warning tests.

**Manual:** run `validate` on each example and read the output.

### Step 10: docs, README, AGENTS.md and skills

- [ ] `docs/configuration-guide.md`: a new shell section (segments, static words, command rules, `paths_under`, construct rules, floors, `[shell]`, evaluation order, reasons, `explain`, zsh notes, known gaps). Update the regex caveats, the shell-chaining note, the worked examples, and the audit section (`kind`, `segment`, floor matches, `shell`). Rule indexes are per kind.
- [ ] `README.md`: a short pitch for compound commands, and `explain`.
- [ ] `AGENTS.md`: the code structure (`src/shell/`), the out-of-scope list (compound commands are now parsed; control structures aren't yet), the brush-parser pinning note, and the corpus and differential test notes. Keep the `legacy_python` heredoc caveat until step 11.
- [ ] `examples/skills/tool-gate-rules-claude/SKILL.md` and `…-copilot/SKILL.md`, following the spec's outline, with valid YAML frontmatter (`name`, `description`). Include the tips below under "For the skills and docs".
- [ ] Test: every TOML snippet in the skills under a "recipe" marker is extracted and loaded with `Config::from_toml` for its agent (`tests/examples.rs`).

**Automated:** the skill-recipe test; the examples test (count updated).

**Manual:**
- Read the guide top to bottom as a new user.
- **Try a skill with a cheap model.** In a scratch project, install the Claude skill. Ask Haiku to (a) allow `npm run lint` and (b) turn a real audit record into a rule. Check the rules are narrow and the model used `explain` to confirm them.

### Step 11: migrate the live config and dogfood

- [ ] Rewrite `~/.config/tool-gate-hook/claude.toml` (and Copilot's) in the new format; `validate`.
- [ ] Replay the corpus: for each line in `corpus.local.jsonl`, compare the old decision (from the audit log) with the new `explain` decision. Review every change of decision, especially new allows.
- [ ] `cargo install --path .`, then use Claude Code for a session with `level = "all"`. Review the audit log for surprising floors or allows.
- [ ] Copilot: run the relevant parts of `docs/copilot-verification.md` with a compound command; fold the results back into fixtures.
- [ ] Remove the `legacy_python` heredoc caveat from AGENTS.md.
- [ ] Fold the remaining spec content into the docs and AGENTS.md, and remove `spec.md` and `plan.md` (as was done after the last rework).

**Verify:** the replay diff is reviewed with no unexplained new allows, and a day of normal use passes without false denies.

## Verification approach (overall)

- **Security-relevant behaviour is tested as acceptance tests** through `tool_gate_hook::run`. Each floor has at least one test showing it overrides a broad allow, and one showing a deny still beats it.
- **Parser agreement** is covered by the zsh/bash differential tests. Shell disagreements are explicit tests that land on floors.
- **Real-world fit** comes from the local corpus (the summary test, plus the step 11 replay) and the committed curated corpus.
- **Readability** is checked by hand with `explain` at each step.
- Coverage follows AGENTS.md: main behaviours and security edges. No concurrency or process orchestration beyond the existing smoke tests and the differential tests.

## Notes for later steps

**For step 5 (where assignments are handled now):**
- In `Walker::simple`, a prefix `AssignmentWord` records `Unsupported("assignment NAME")` and only walks the substitutions in its value; suffix assignments (`export X=1`) are plain args. Replace this with `env` entries plus the `EnvAssign` floor, and classify the value with `self.word(…)` (probably a new `Role` so the detail reads e.g. `variable in env FOO`) so that `FOO=$x cmd` hits `expansion`.
- An assignment-only command (`word_or_name: None`) currently gets two floors: `assignment X` and `no command word in …`. It should become a neutral segment instead; keep `no command word` only for redirect-only commands (`> file`), which stay `unsupported` for good.
- Neutral segments need a segment kind: the combiner's "every segment allowed" check skips them, and the audit shows `"decision": "neutral"` (`segment_decisions` is `Vec<Option<Decision>>` today).
- `wrappers` is only filled with `time` today (in `Walker::commands`). Unwrap after the name and args are computed as static strings.

**Example and test facts:**
- `cargo test > /etc/x` is still allowed by the examples' cargo rule, as it was by the old regex rule; step 8's `redirect_write` rule fixes it.
- In audit records, `[[rule]]` matches come before command-rule matches. `tests/audit.rs` `deny_wins_and_all_matches_are_recorded_rules_first` pins this.
- The mermaid example has no `echo $$-$RANDOM` rule any more; `$` is a floor, and its test asserts it asks.

**For the skills and docs:**
- About a quarter of real commands ask because of shell variables (`$f`, `$d`), globs (`docs/*.md`), `for` loops and `S=…` assignments. Agents should spell paths out when they want a command auto-allowed.
- `echo ====` is allowed, but under zsh (Claude Code's shell) it fails with "=== not found" and skips the rest of the line, so agents should use `echo ---`.
- To `explain` a command from an audit record, pipe the line in: `tail -1 audit.jsonl | tool-gate-hook explain --agent claude --payload -`.
