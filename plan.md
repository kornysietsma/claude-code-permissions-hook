# Plan: shell-aware rules

Implements [`spec.md`](./spec.md) in vertical slices. Each step leaves the tool working, passes the quality gate, and can be checked by hand. Tick the boxes as steps land, and record anything later work needs in [Notes](#notes).

Quality gate, for every change:

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

## Status

- **Branch** `shell-parsing`, not pushed; the PR is raised at the end. `git log` shows a commit per step.
- **Done:** steps 1 to 10. The feature is complete and documented: parsing, segments, command rules, `paths_under`, `cd` tracking, construct rules, all floors, `[shell] safe_env`, wrappers, the audit `shell` object, `explain`, `validate`, the examples, the configuration guide (with a worked Mermaid diagram and tables of contents), README, AGENTS.md and the two rule-writing skills.
- **Next:** step 11 dogfooding. The live config is migrated (old one at `~/.config/tool-gate-hook/claude.toml.pre-shell-parsing`) and the new binary installed on 2026-10-08, with `max_value_len = 0` for replayable logs; the "Working on this branch" cautions below no longer apply.
- **Deferred (the author, later):** try the Claude skill with a cheap model: in a scratch project, install `examples/skills/tool-gate-rules-claude`, ask Haiku to (a) allow `npm run lint` and (b) turn a real audit record into a rule; check the rules are narrow and that it used `explain`.

## Working on this branch

These hold until step 11 installs the new binary:

- **Don't `cargo install` before step 11's migration.** The author's live config (`~/.config/tool-gate-hook/claude.toml`) has Bash allow `[[rule]]`s, which are now a config error, so the new binary would `ask` on every call.
- **The live hook is still the old binary.** Its `legacy_python` rule denies any Bash command whose text contains a Python tool name where a command could start, including inside heredocs, `$( )` and `<( )`. So:
  - write files containing such text with the Write/Edit tools, not `cat <<EOF`, perl or sed;
  - to `explain` such a command, put a payload in a scratchpad file and use `--payload`;
  - write commit messages to a file in the scratchpad and use `git commit -F`.

These hold regardless:

- **Something between the agent and the file turns a backslash-u unicode escape in written source into the literal character.** Build such strings at runtime, e.g. `format!("$'{}u00e9'", '\\')`.
- **Claude Code's built-in `rm` safety check blocks inline `zsh -c '…'` / `bash -c '…'` scripts run from the Bash tool**, even without `rm`. Write a script file to the scratchpad and run `zsh -f FILE`.
- **The Bash tool runs zsh with `EQUALS` on**, so `echo ====` fails there. Use `echo ---` as a separator.
- **`explain`** is the quickest manual check: `cargo run -q -- explain --agent claude --config examples/claude.toml --cwd /tmp 'COMMAND' | jq …`. An empty config file shows the bare analysis.

## Step 11: migrate the live config and dogfood

- [x] Rewrite `~/.config/tool-gate-hook/claude.toml` in the new format (there is no live Copilot config on this machine; Copilot runs on a work machine, see `docs/copilot-verification.md`), using `examples/claude.toml` as the model: Bash allows become `[[command_rule]]`s on `text` (file-touching ones with `paths_under`), `shell_chain` / `parent_dir` patterns and `not_regex` safety nets go, `legacy_python` becomes a deny on `name`, and add the construct rules, the harmless-builtins allow and `[shell] safe_env`. Show the author the new config before writing it; `validate` it.
- [x] Replay the corpus: for each Bash call in the live audit log (`~/.local/share/tool-gate-hook/claude.jsonl`), compare the old `decision` with the new config's `explain` decision. Review every change, especially new allows, with the author. See [Corpus tools](#corpus-tools).
- [ ] (installed 2026-10-08; dogfooding pending) `cargo install --path .`, then use Claude Code for a session with `level = "all"`. Review the audit log for surprising floors or allows. After this, the "Working on this branch" cautions about the old binary no longer apply.
- [ ] Copilot: run the relevant parts of `docs/copilot-verification.md` (its script already uses command rules) with a compound command, e.g. `echo tgh-allow && echo tgh-allow`; fold the results back into fixtures and `docs/copilot-tool-inputs.md`.
- [ ] Remove the `legacy_python` heredoc caveat from AGENTS.md ("Project knowledge").
- [ ] Fold anything in `spec.md` not yet in the docs or AGENTS.md into them, then remove `spec.md` and `plan.md` (as was done after the last rework). Most of the spec is already in `docs/configuration-guide.md` and AGENTS.md; check especially the design rationale (why floors exist, why a `cd` never removes a directory, why rule indexes are per kind).
- [ ] Raise the PR from `shell-parsing` to `main`.

**Verify:** the replay diff is reviewed with no unexplained new allows, and a day of normal use passes without false denies.

## Corpus tools

- **Local corpus** (gitignored, may contain private paths): `scripts/shell-corpus.sh ~/.local/share/tool-gate-hook/claude.jsonl` writes `tests/fixtures/shell/corpus.local.jsonl` (one JSON string per line: the command). `cargo test --test shell_corpus -- --ignored --nocapture` lists each floor's details per command. At step 9, 162 commands, of which about three quarters hit no floor.
- **For the replay**, skip records whose command ends in `…[truncated, N chars]` (`max_value_len = 1024` cuts long heredocs, which then fail to parse). the old decision is in the audit log itself: `jq -c 'select(.payload.tool_name == "Bash") | {command: .payload.tool_input.command, decision, cwd: .payload.cwd}' ~/.local/share/tool-gate-hook/claude.jsonl`. For each line, build a payload with `jq` (commands can contain newlines, so don't pass them as arguments), run `explain --payload`, and print old and new decision with the reason. Run it from a script file with `zsh -f`, as below. Use the record's own `cwd`, so `paths_under` and `cd` resolve as they did.
- **Curated corpus expectations** (`tests/fixtures/shell/corpus.jsonl`, checked by `tests/shell_corpus.rs`) are regenerated with `explain`, an empty config and `--payload`, then reviewed with `diff`. After `cargo build`, save this as `regen.sh` in the scratchpad (`$S`) next to an empty `empty.toml`, run `zsh -f $S/regen.sh $S > $S/corpus.new.jsonl`, and `diff` it against `tests/fixtures/shell/corpus.jsonl`:

  ```zsh
  S=$1
  while IFS= read -r line; do
    print -r -- "$line" | jq -c '{hook_event_name:"PreToolUse",cwd:"/tmp",tool_name:"Bash",tool_input:{command:.command}}' > $S/payload.json
    cmd=$(print -r -- "$line" | jq -c '.command')
    ./target/debug/tool-gate-hook explain --agent claude --config $S/empty.toml --payload $S/payload.json \
      | jq -c --argjson cmd "$cmd" '{command:$cmd, names:[.shell.segments[].name], floors:([.shell.constructs[] | select(.floor) | .construct] | unique)}'
  done < tests/fixtures/shell/corpus.jsonl
  ```

  The test analyses with cwd `/tmp` and home `/home/me`; `explain` uses the real home, so a command that `cd`s under the home could differ.

## Notes

For fixing anything dogfooding turns up. AGENTS.md has the code structure; `docs/configuration-guide.md` the user-facing behaviour.

**Author's preferences:**
- Keep things simple rather than adding code for obscure edge cases. Raise real security holes as questions (as with the `cd` scoping), but don't build for unlikely ones.
- `printf -v`, `read`, `mapfile`/`readarray` and `getopts` set variables; they are handled only by keeping them out of allow rules, not by code.
- brush's `source` rendering (`2>& 1`, `<(( x ))`) is left as is; reasons cut quoted text to its first line and 100 characters (`policy::brief`).

**Design decisions:**
- **Floors only veto allows** (decided at step 11): a floor turns an allow into an ask, and is ignored when some segment isn't allowed. Replaying the live log with floors always asking and no allows, 119 of 505 Bash calls (24%) went from passthrough to ask in auto mode; with the veto, none did. So a parse error, a bare `for` loop or a missing command string passes through, since there are no allowed segments. When a rule asks or denies, that rule is the deciding item, not a floor.
- **Construct rules may name floors** (step 11 review): they ask or deny whenever the floor fires, allowed or not. This keeps goal 2 (catch what a classifier or a misled agent lets slip) for the floors that hide commands from deny rules: the examples and the live config ask on `shell_reentry`, `exec_tool` and `dynamic_command`, but not `unsupported` (mostly harmless `for` loops). The docs say plainly that the hook isn't a sandbox (`./x.sh` gets past every command rule).
- **The live config has no allows** (author's choice): it denies legacy Python, asks on `background`, `redirect_write` outside `{cwd}`, `/tmp`, `/private/tmp`, `/dev/null`, and the three floors above (19 of 568 replayed calls, all `zsh -f` / `bash` scripts), and has `safe_env` ready for later allows. Allows get added when a frequent command isn't caught by the auto-mode classifier.
- **Two-level parsing.** `brush_parser::Parser::new(Cursor::new(cmd), &ParserOptions::default()).parse_program()` gives the AST, where words are raw strings; `brush_parser::word::parse(raw, &options)` splits a word into pieces. Default options are bash mode with extended globbing on.
- **Allow-listed walker with exhaustive matches** (no `_ =>` arms); brush-parser is pinned to `=0.4.0`.
- **Segment order** is textual, except that a command comes before the commands substituted into it (substitutions are queued as `Pending` and walked after the segment is pushed).
- **Segments are matched as JSON** through the existing `FieldCondition` code, with a `Location { cwd, dirs }`: `{cwd}` is always the payload cwd, and a relative path must pass from every one of `dirs`.
- **Possible directories**: the walker's `dirs` starts as the cwd, and each neutral `cd` adds its target resolved from every directory already in it; nothing is removed. Redirect constructs record `dirs` when seen, which also covers a group's own redirects (no segment).
- **Combiner**: deny or ask if any match has it (the first most restrictive match decides); else allow if there is at least one non-neutral segment and all are allowed; else passthrough. Match order: `[[rule]]`s, floors, command rules by segment, construct rules.
- **Rule indexes are per kind**, because TOML deserialises each array of tables separately.
- **serde_json has `preserve_order`**, so records and `explain` output keep struct order.

**brush-parser facts:**
- Any `NAME=value`-shaped word is an `AssignmentWord` (in the prefix for `FOO=1 cmd`, and in the suffix for `make CC=gcc` too), so the walker decides by command name. An assignment-only command has `word_or_name: None`.
- `time` is `Pipeline.timed`; `!` is `Pipeline.bang`. Redirects carry no source location; `source` is the `Display` rendering of `SimpleCommand`. Parse errors come back as `Err`. A quoted heredoc delimiter arrives raw (`'EOF'`).

**Shell analysis details:**
- Wrappers are unwrapped only when the name is bare; when unwrapping fails, the segment keeps the wrapper as its name. `shell_reentry`, `exec_tool` and `cd` checks use the unwrapped name's basename.
- `cd_targets()` treats a `cd` argument as non-static if any floor was already recorded for that segment.
- A non-static word after a wrapper (`timeout 60 $CMD`) becomes the name but hits `expansion`, not `dynamic_command`.
- bash (not zsh) tilde-expands after `=` in assignment-like args; the segment shows the literal `~`. Only the `--opt=value` part is path-like, so `paths_under` is unaffected.

**Test facts:**
- `tests/shell.rs` has the shell acceptance tests with a shared `CONFIG` (append new rules at the end: reason assertions use rule numbers), `ALLOW_EVERYTHING` / `decision_allowing_everything` for floor tests, and `Project` (a temp project with `sub/sub2`, a `link` to outside, and a home) with `decision` / `decision_with(config, command)` for `paths_under`, `cd` and construct rules. Put helpers there rather than in `tests/common/mod.rs`, where an unused function is a dead-code error in other test binaries.
- `tests/examples.rs` checks the examples (with their exact `validate` summaries) and the skills' recipes (fenced as ```` ```toml recipe ````, loaded for their agent, no warnings).
