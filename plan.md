# Plan: shell-aware rules

Implements [`spec.md`](./spec.md) in vertical slices. Each step leaves the tool working, passes the quality gate, and can be checked by hand. Tick the boxes as steps land, and record anything later steps need in [Notes for later steps](#notes-for-later-steps).

Quality gate, for every step:

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

## Status

- **Branch** `shell-parsing`, not pushed; the PR is raised at the end. The last commit is step 7 (`git log` has the hash).
- **Done:** steps 1 to 7.
- **Next:** step 8, construct rules.

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
  - after step 7, 120 of 162 commands hit no floor (a `cd` floor only adds to commands that already had one). Most floors are `$VAR` from `S=…` assignments (20 `env_assign`), and globs. The 8 `shell_reentry` floors are all `bash script.sh` or `bash -n`.
- **Corpus expectations** are regenerated with `explain`, an empty config and `--payload` (build each payload with `jq`, as commands can contain newlines), then reviewed with `diff`. After `cargo build`, save this as `regen.sh` in the scratchpad (`$S`) next to an empty `empty.toml`, run `zsh -f $S/regen.sh $S > $S/corpus.new.jsonl`, and `diff` it against `tests/fixtures/shell/corpus.jsonl`:

  ```zsh
  S=$1
  while IFS= read -r line; do
    print -r -- "$line" | jq -c '{hook_event_name:"PreToolUse",cwd:"/tmp",tool_name:"Bash",tool_input:{command:.command}}' > $S/payload.json
    cmd=$(print -r -- "$line" | jq -c '.command')
    ./target/debug/tool-gate-hook explain --agent claude --config $S/empty.toml --payload $S/payload.json \
      | jq -c --argjson cmd "$cmd" '{command:$cmd, names:[.shell.segments[].name], floors:([.shell.constructs[] | select(.floor) | .construct] | unique)}'
  done < tests/fixtures/shell/corpus.jsonl
  ```

  The test analyses with cwd `/tmp` and home `/home/me`; `explain` uses the real home, so a command that `cd`s under the home could differ (none do yet).
- **`explain`** is the quickest manual check: `cargo run -q -- explain --agent claude --config examples/claude.toml --cwd /tmp 'COMMAND' | jq …`. An empty config file shows the bare analysis.

## Technical context

### Code shape (as of step 7)

| Module | Contents | Still to come |
|--------|----------|---------------|
| `src/shell/mod.rs` | `analyse(command, home, cwd, settings) -> Analysis`; `Settings { safe_env }` (the `[shell]` config); the allow-listed `Walker` (with `depth`, capped at `MAX_DEPTH` = 16); `Segment` (with `kind: SegmentKind`, `#[serde(skip)]`, and `dirs: Vec<PathBuf>`, serialised last and only when there is more than the cwd; `is_neutral()`; `path_like() -> Option<Vec<&str>>`), `SegmentKind::{Command, AssignmentOnly, Cd}`, `Redirect`, `Construct { kind, segment, detail, target }`, `ConstructKind` (floors `ParseError`, `Unsupported`, `Expansion`, `DynamicCommand`, `EnvAssign`, `ShellReentry`, `ExecTool`, `Cd`; recorded `Substitution`, `Heredoc`, `Pipe`, `Background`, `Subshell`, `RedirectRead`, `RedirectWrite`; `name()`, `is_floor()`, `describe()`); `Analysis::unsupported(detail)`. Words and redirects queue `Pending` substitutions, which `walk_pending` walks after the segment is pushed. `enclosing()` records pipe, background and subshell against the first segment inside. A word's `Role` (command, arg, redirect, here-string) picks the floor and the detail wording; `value(raw, shown, …)` classifies `raw` but reports `shown` (an assignment's value is reported as the whole `NAME=value`). `simple()` puts prefix assignments in `env` (`assignment()`), checks names with `check_env_name()` (prefix, `env NAME=…`, and the `DECLARATIONS` builtins' args via `assigned_name()`), and calls `unwrap()`, then `command_floors()` and `cd()` (`cd_targets()` adds to the walker's `dirs`, capped at `MAX_DIRS` = 16; also `SHELL_REENTRY` names and `xargs`, and `find` with `FIND_ACTIONS`, by the basename of the unwrapped name). `file_redirect()` records `redirect_read`/`redirect_write`. | Recording `dirs` on a group's own `redirect_write` constructs (8). |
| `src/shell/wrappers.rs` | `unwrap(name, args) -> Result<Unwrapped { wrappers, env, name, args }, String>`: peels `env`, `timeout`, `nice`, `nohup` and `time` (bare names only), with each one's option grammar; `Err` is the `unsupported` detail. Unit tests. | |
| `src/shell/words.rs` | `analyse(raw, home, options) -> WordInfo { value: Result<String, NotStatic>, substitutions }`, `analyse_heredoc(raw, options)`; `NotStatic::{Expansion, Unsupported}(why)`; `$'…'` decoding; `quote()` for `text`. Unit tests. | |
| `src/agent.rs` | `Agent::shell_tool() -> ShellTool { name, command_path }`; `Agent::shell_payload(command, cwd)`, a minimal payload used by `explain` and tests. | |
| `src/config.rs` | `Config::load(path, agent)` / `from_toml(contents, agent)`; `[shell]` (`RawShell`, `safe_env` compiled with `compile_regexes`, so `@patterns` work); `[[command_rule]]` with `paths_under`; the errors for an allow `[[rule]]` that can match the shell tool, for a command rule with no `match` and no `paths_under`, and for an empty `paths_under`. | `[[construct_rule]]` (8). |
| `src/policy.rs` | `Policy { rules, command_rules, shell_tool, shell: shell::Settings }`; `RuleKind` (`Rule`, `CommandRule`, `Floor`); `Rule` (reused for command rules, `tool: None`; `paths_under` empty for `[[rule]]`s); `Location { cwd, dirs }` and `is_under`; `Rule::matches_segment` (fields, then `paths_under`); `Match { kind, index: Option, decision, description, reason, segment: Option (1-based) }`; `Evaluation { matches, decided_by, shell: Option<ShellEvaluation { analysis, segment_decisions: Vec<Option<Decision>> }> }` (`None` for neutral segments, which rules never see); the combiner in `Policy::evaluate` skips neutral segments and needs at least one non-neutral one to allow; `floor_match`. | `ConstructRule` kind and evaluation (8). |
| `src/auditing.rs` | `AuditRecord::for_evaluation(invocation, payload, evaluation, max_value_len)` builds the record, including the `shell` object (`ShellRecord`, serialised to a `Value` and truncated; a neutral segment's `decision` is `"neutral"`); the caller checks the level with `AuditConfig::records`. `AuditRecord::for_explanation(…, truncated)` adds `reason`. `is_truncated`, `truncate_json_strings`. | |
| `src/validate.rs` | Summary lines for rules and command rules. | Construct rules, `safe_env`, segment-field warnings (9). |
| `src/lib.rs`, `src/main.rs` | `run`, `validate`, and `explain` (`lib::explain(agent, config_path, input, context) -> Result<Explanation { record, warnings }>`; the input is a payload or an audit record; `--payload -` reads stdin). | |

Tests:

| File | Contents |
|------|----------|
| `tests/shell.rs` | Acceptance tests through `run` for both agents, with one shared `CONFIG` (it has `[patterns]` and `[shell] safe_env` for `RUST_LOG`, `RUST_BACKTRACE` and `NO_COLOR`; rule numbers are referenced in reason assertions, so append new rules at the end). Its `shell_payload(agent, command)` helper wraps `Agent::shell_payload` with cwd `/tmp`. Also `ALLOW_EVERYTHING` with `decision_allowing_everything(command)` (a match-all allow plus an `rm -rf` deny, for "floor beats a broad allow" and "deny beats the floor" tests), and `PATHS_CONFIG` with `Project` (a temp dir with `project/sub/sub2`, `project/link` → outside, and `home/scratch`; `Project::decision(command)` runs with that cwd and home) for `paths_under` and `cd`. Put helpers here rather than in `tests/common/mod.rs`, where an unused function is a dead-code error in other test binaries. |
| `tests/shell_differential.rs` | `AGREED` (static commands zsh, bash and we split identically) and `DISAGREED` (must hit a floor). |
| `tests/shell_corpus.rs` | `curated_corpus_is_analysed_as_expected` checks `tests/fixtures/shell/corpus.jsonl`: sanitised real commands with expected segment names and floors. To add cases, generate the expectations with `explain`, an empty config and `jq`, then review them. Plus the ignored local-corpus summary. |
| `tests/audit.rs` | Audit records, including the full `shell` shape (`shell_record_shows_each_segment_and_construct`), neutral segments, `dirs` after a `cd`, and truncation inside `shell`. |
| `tests/config.rs` | Config parsing and errors, including `[shell]` and `paths_under`. |
| `tests/explain.rs` | `lib::explain`: both input forms, both agents, reasons, truncated records, errors. `tests/smoke.rs` spawns the `explain` CLI. |
| `tests/examples.rs` | The example configs, including the legacy-python cases (denied inside `$( )`, backticks, `<( )` and unquoted heredocs; not denied in quoted text). |

### Design decisions

- **Two-level parsing.** `brush_parser::Parser::new(Cursor::new(cmd), &ParserOptions::default()).parse_program()` gives the AST, where words are raw strings (`ast::Word { value, loc }`). `brush_parser::word::parse(raw, &options)` splits a word into `WordPiece`s. Default options are bash mode, with extended globbing on: `@(…)` parses as text, which the glob check catches.
- **Allow-listed walker with exhaustive matches** on brush's enums (no `_ =>` arms), so a brush upgrade that adds syntax fails to compile instead of being silently allowed. brush-parser is pinned to `=0.4.0`.
- **Segment order**: textual, except that a command comes before the commands substituted into it (substitutions are queued as `Pending` and walked after the segment is pushed, so segment indexes stay stable).
- **Segments are matched as JSON**: `serde_json::to_value(&segment)` goes through the existing `FieldCondition` code. `FieldCondition::matches(value, location, context)` takes a `Location { cwd, dirs }`: `{cwd}` in configured directories is always the payload cwd, and a relative path must pass from every one of `dirs` (a segment's possible directories, or just the cwd for `[[rule]]`s). `Location::is_under` is shared by `under` and `paths_under`.
- **Combiner** (`Policy::evaluate`): deny or ask if any match has it (the first most restrictive match decides); else allow if there is at least one non-neutral segment and every non-neutral segment's decision is allow (the first allow match decides); else passthrough. Neutral segments (assignment-only, in-project `cd`) are never shown to command rules.
- **Possible directories**: the walker's `dirs` starts as the cwd, and each neutral `cd` adds its target resolved from every directory already in it; nothing is ever removed. This one rule covers failed `cd`s, `||`, subshells, substitutions and pipelines. Construct rules (step 8) must check `redirect_write` targets the same way.
- **Rule indexes are per kind**, because TOML deserialises each array of tables separately and loses their relative order.
- **Floor details** are short and specific (`for loop`, `variable in $HOME`, `assignment FOO`), and they appear in reasons.
- **serde_json has `preserve_order`**, so records and `explain` output keep struct order.

### brush-parser facts needed for later steps

- **Assignments**: `CommandPrefixOrSuffixItem::AssignmentWord(Assignment { name: AssignmentName, value: Scalar(Word) | Array(..), append, .. }, Word)`, where the `Word` is the whole `NAME=value`. In the prefix for `FOO=1 cmd`; in the suffix for **any** `NAME=value`-shaped arg (`make CC=gcc`, `echo a=b`), not just declaration builtins, so the walker decides by command name. An assignment-only command is a `SimpleCommand` with `word_or_name: None`.
- **`time`** is `Pipeline.timed` (`Timed` or `TimedWithPosixOutput` for `time -p`); **`!`** is `Pipeline.bang`. Pipeline items are `(PipelineOperator, &Pipeline)` from `AndOrList::iter()`. `PipelineOperator` has no `Debug`.
- **Redirects** carry no source location; **source spans** are character indexes, so use the `Display` rendering of `SimpleCommand` for `source`.
- **Parse errors** come back as `Err` (never seen to panic).
- A quoted heredoc delimiter arrives raw (`'EOF'`); the walker unquotes it.

### Things to keep in mind throughout

- `run` must always exit 0. Nothing in the shell analysis may panic, so no `unwrap` on parser output.
- Keep the payload a `serde_json::Value`.
- Every floor needs a test showing it beats a broad allow, and one showing a deny still beats the floor.

## Steps

### Steps 1–7 ✅

1. **brush-parser spike.** brush-parser 0.4.0 is in.
2. **First end-to-end slice.** Segments; command rules on `text`/`name`; the `parse_error` and `unsupported` floors; an allow `[[rule]]` on the shell tool is a config error; the examples migrated; differential tests against zsh and bash.
3. **Audit `shell` object and `explain`.** Including the `reason` in `explain` output, and `ask` for truncated audit records.
4. **Substitutions, expansions, dynamic command names.** Word classification; the `expansion` and `dynamic_command` floors; recursion into substitutions (depth 16); unquoted heredoc bodies; non-floor constructs recorded; the curated corpus; `====`-style words are static.
5. **Wrappers, `[shell] safe_env`, `env_assign`.** Prefix and `env` assignments go to `env`; the `env_assign` floor (also for declaration builtins' args); assignment-only segments are neutral; array assignments are `unsupported`; `env`/`timeout`/`nice`/`nohup`/`time` unwrapped (bare names only); `safe_env` in the claude and copilot examples.
6. **`shell_reentry` and `exec_tool` floors.** By the basename of the unwrapped name, so `env bash x` and `/bin/bash x` are caught; `find` only with an action that runs commands or deletes.
7. **`cd` tracking and `paths_under`.** Segments carry every directory the shell might be in (a `cd` adds, never removes, since it can fail or be scoped to a subshell); in-cwd `cd`/`pushd` is neutral and anything else the `cd` floor; `paths_under` on command rules (URLs aren't path-like; `-o/etc/x`-style args fail the rule); `under` on segment fields checks every possible directory; the mermaid example uses `paths_under` and `parent_dir` is gone.

### Step 8: construct rules

- [ ] Config: `[[construct_rule]]` with `decision`, `construct`, `description`, `reason`, and `outside` (only for `redirect_write`). Errors: allow, an unknown construct, a floor name, `outside` on another construct. Add `RuleKind::ConstructRule`.
- [ ] Evaluation: a construct rule matches if any recorded construct of its kind is present. For `redirect_write` with `outside`, only targets not under those directories count, resolved from every one of the segment's possible directories with `Location::is_under`. A group's own redirects (`( … ) > out`) have no segment, so record the walker's `dirs` at that point on the construct (they are applied before the group runs). Matches go after the command-rule matches.
- [ ] Examples: ask on `background`; ask on `redirect_write` outside `{cwd}` and `/tmp`. Add the harmless-builtins allow (`true`, `false`, `:`, `echo`, `printf`, `test`, `[`), held back from step 2 until writes are checked.

**Automated:**
- `cargo test > /tmp/out.txt` → allow; `cargo test > ~/out.txt` → ask; `cargo test > out.txt` → allow;
- `echo x > ~/.zshrc` → ask with the example config;
- `cd sub && cargo test > ../../x` → ask; `cd sub && cargo test > ../x` → ask too (from the cwd it is outside);
- a group's own redirect: `(cd sub && cargo test) > ../x` → ask, checked from the directories before the group;
- `cargo test > /etc/x` → ask with the example config (it was allowed until now);
- `cargo test &` → ask;
- the harmless builtins: `echo hi && true` → allow; `printf -v PATH x` and `read PATH` → not allowed;
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

**Shell analysis facts:**
- An assignment-only segment has empty `text` and `name`; a `cd` segment keeps them.
- Wrappers are unwrapped only when the name is bare (`env`, not `/usr/bin/env` or `./env`, which could be anything). An absolute-path wrapper stays the command name, so it passes through unless a rule names it. `shell_reentry`, `exec_tool` and `cd` checks run on the unwrapped name, so `env bash -c …` is caught, and `env cd x` is a `cd` floor (an external `cd` changes nothing).
- `cd_targets()` treats a `cd` argument as non-static if any floor was already recorded for that segment.
- When unwrapping fails (unknown option), the segment keeps the wrapper as its name, with no `wrappers`.
- A non-static word after a wrapper (`timeout 60 $CMD`) becomes the name, but hits `expansion` (not `dynamic_command`), since it was classified as an arg.
- **For step 8's harmless builtins:** `printf -v NAME …` and `read NAME` also set variables (e.g. `printf -v PATH ./evil; cargo test`). The `printf` allow must exclude `-v` (e.g. `match.text = { regex = '^printf\b', not_regex = '^printf -v\b' }`, or a separate ask rule), and `read`, `mapfile`/`readarray` and `getopts` must not be in it. Decided (by the author): handle this only by keeping them out of the allow rule. Don't add code to treat them as assignments; it's not worth it for such obscure cases.
- **Author's preference:** keep things simple rather than adding code for obscure edge cases; raise real security holes as questions (as with the `cd` scoping), but don't build for unlikely ones.
- bash (not zsh) tilde-expands after `=` in assignment-like args (`make CC=~/x` passes `CC=/home/…/x`); the segment shows the literal `~`. Only the `--opt=value` part is path-like, and `--opt` isn't a valid name, so `paths_under` is unaffected; keep it that way.
- **For step 9:** review the `env_assign` description ("a variable not listed in [shell] safe_env") with the other floor texts. Floor reasons repeat themselves when the segment is only the name, e.g. `… (sh), in "sh"` for `git diff | sh`.

**Example and test facts:**
- `cargo test > /etc/x` is still allowed by the examples' cargo rule, as it was by the old regex rule; step 8's `redirect_write` rule fixes it.
- In audit records, `[[rule]]` matches come before command-rule matches. `tests/audit.rs` `deny_wins_and_all_matches_are_recorded_rules_first` pins this.
- The mermaid example has no `echo $$-$RANDOM` rule any more; `$` is a floor, and its test asserts it asks.

**For the skills and docs:**
- About a quarter of real commands ask because of shell variables (`$f`, `$d`), globs (`docs/*.md`), `for` loops and `S=…` assignments. Agents should spell paths out when they want a command auto-allowed.
- `echo ====` is allowed, but under zsh (Claude Code's shell) it fails with "=== not found" and skips the rest of the line, so agents should use `echo ---`.
- To `explain` a command from an audit record, pipe the line in: `tail -1 audit.jsonl | tool-gate-hook explain --agent claude --payload -`.
- After a `cd`, a path is checked from every directory the shell might be in, so `cd sub && rm ../x` never passes `paths_under`. Agents should use paths relative to the project root, or `cd` and then stay below it.
- Known gap to document: `CDPATH` / zsh `cdpath` in the user's shell config can send `cd sub` elsewhere; the hook assumes it isn't set.
- A scoped npm package (`npx -p @scope/pkg …`) contains `/`, so it counts as path-like and fails `paths_under`. The mermaid example's npx rule has no `paths_under` for this reason.
