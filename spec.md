# Spec: shell-aware rules for the shell tool

## Summary

Today, rules for `Bash` (Claude) and `bash` (Copilot) match the whole command string with regexes. Allow rules need a `not_regex = "@shell_chain"` safety net, so `foo && bar` can never be auto-allowed even when both parts are fine, and deny rules such as `legacy_python` fire on quoted text (heredocs, commit messages).

This change parses shell commands into a syntax tree, splits them into **segments** (simple commands), and evaluates:

- **command rules** (`[[command_rule]]`) against each segment,
- **construct rules** (`[[construct_rule]]`) against risky shell structure (redirects, backgrounding),
- **floors**: built-in checks that force at least `ask` for things that can't be validated statically (shell re-entry, expansions, dynamic command names, unknown syntax, ...).

A shell call is allowed only when **every** segment is allowed by a command rule and nothing asks or denies. Shell parsing is always on; there is no opt-in switch. Backward compatibility with the current config format is **not** a goal (single user, just released): existing configs and examples are rewritten.

## Goals

- Auto-allow compound commands (`cargo build 2>&1 && cargo test`) when every part is allowed.
- Make quoting meaningful: a heredoc line starting `pip` is data, not a command.
- Turn risky constructs into structural facts, mostly resulting in `ask` so a human checks.
- Never allow something the parser doesn't fully understand.
- Keep the TOML simple: regexes on a normalised command string, plus one path check.
- Make long, messy commands easy to sense-check, via the audit log and an `explain` command.
- Ship copyable skills that let cheaper models write rules for this tool.

## Non-goals

- Argument-level matching DSL (quantifiers over args, flag/operand parsing). Fields exist in the segment object and are reachable by field path, but are not documented as a rule-writing surface.
- Tracking variable values, or evaluating expansions.
- Unwrapping `xargs`, `find -exec`, `sudo` or arbitrary wrappers.
- Parsing PowerShell, or zsh-specific syntax (it hits a floor instead).
- Compatibility with the current config format for shell-tool allows.

## Concepts

### Shell tool

Defined by the agent adapter (`src/agent.rs`), not by config:

| Agent | Tool name | Command field |
|-------|-----------|---------------|
| claude | `Bash` | `tool_input.command` |
| copilot | `bash` | `toolArgs.command` |

Copilot running `.claude/settings.json` hooks sends Claude-format payloads; with `--agent claude` those are handled as Claude `Bash` calls (already the case).

If the shell tool's command field is missing or not a string, the call is evaluated as `unsupported` (ask floor) after `[[rule]]` deny/ask checks.

### Target shells: zsh and bash

Claude Code runs `Bash` tool commands in the user's shell, which on macOS is **zsh**; Copilot's `bash` tool uses bash. Commands are parsed with a bash grammar (brush-parser). For the static simple commands that can be allowed, zsh and bash agree; where they differ, the difference must land on a floor:

- Unquoted `$var` isn't word-split in zsh: irrelevant, any parameter expansion is already `expansion`.
- zsh glob qualifiers (`*(e:'cmd':)`) contain a glob (`expansion`) or are a bash syntax error (`parse_error`).
- zsh **equals expansion**: an unquoted word starting with `=` (`=python3`) expands to a command path. Counted as `expansion`.
- zsh-only syntax (`${(f)x}`, `print -r --`, …) fails to parse or contains an expansion.
- Shell-state builtins that change later segments (aliases, options, traps) are `shell_reentry` floors (below).

Differential tests compare our word splitting with both `zsh -f` and `/bin/bash` (macOS ships bash 3.2).

### Segment

A segment is one simple command found anywhere in the tree: in lists (`;`, `&&`, `||`, newline), pipelines, subshells `( … )`, brace groups `{ …; }`, command substitutions `$( … )` and backticks, and process substitutions `<( … )` / `>( … )` (recursion). Control structures (`if`, `for`, `while`, `case`, functions) are **unsupported** in the first version (ask floor); they can be added to the walker later.

Each segment is exposed as a JSON object, used both for rule matching and in the audit record:

```json
{
  "text": "./scripts/thing.py --out 'my file.txt'",
  "name": "./scripts/thing.py",
  "args": ["--out", "my file.txt"],
  "env": { "FOO": "1" },
  "redirects": [
    { "op": ">&", "fd": 2, "target": "1" },
    { "op": ">", "target": "log.txt" }
  ],
  "wrappers": ["env"],
  "source": "FOO=1 ./scripts/thing.py --out \"my file.txt\" 2>&1 > log.txt"
}
```

- `name`: the command word after removing quotes, after unwrapping wrappers. A leading `~` is expanded to the home directory.
- `args`: words after removing quotes, split as bash would. A leading `~` / `~/` is expanded statically.
- `text`: `name` and `args` joined by spaces, each re-quoted only if needed: plain if it matches `[A-Za-z0-9_@%+=:,./-]+`, else single-quoted with `'\''` for embedded quotes (`''` for an empty word). It doesn't include env, redirects or wrappers. **Rules match this field.** `^` means "start of this command".
- `env`: inline assignments plus assignments from an unwrapped `env`.
- `redirects`: operator (`<`, `>`, `>>`, `<>`, `>|`, `<&`, `>&`, `&>`, `&>>`, `<<`, `<<-`, `<<<`), optional fd, and target (unquoted; the delimiter for heredocs). Redirects on a `( … )` or `{ …; }` group are copied onto every segment inside it.
- `wrappers`: the unwrapped wrapper names, in order (audit only). The `time` keyword (`time cargo test`) is recorded here too.
- `source`: the segment as the parser renders it (audit and reasons only). It is normalised, not a slice of the input, e.g. `>log.txt` becomes `> log.txt`.

A pipeline's `!` is ignored. A segment with no command word and only redirects (`> file`) is `unsupported`.

#### Static words

A word is static when its value can be known without running the shell. Its value is built from the parser's pieces:

- unquoted text, single-quoted text, and double-quoted text and escapes are literal. Outside single quotes, `\x` is `x`; inside double quotes, only bash's escapes are removed (`"a\b"` stays `a\b`);
- `$'…'` is decoded for `\n \t \r \a \b \e \E \f \v \\ \' \" \?`, `\xHH` and octal, and only to ASCII. Any other escape (`\u`, `\U`, `\c`, …) is `unsupported`;
- a leading `~` (alone or before `/`) is the home directory. `~user`, `~+` and `~-` are `expansion`;
- anything else (parameter, arithmetic and command substitution, `$"…"`) is `expansion`.

Unquoted text (where quoted parts count as plain characters) is also `expansion` if it contains `$` (brush reads zsh's `${(f)x}` as the text `$` + `{(f)x}`), `*` or `?`, `[` with a later `]`, an extended glob (`+(`, `@(`, `!(`), or a brace expansion (`{…,…}` or `{…..…}`), or if it starts with `=` followed by anything (zsh equals expansion; a lone `=` is static). So `{}`, `HEAD@{1}`, `[` and `a=b` are static.

### Wrappers (built-in unwrapping)

`env`, `timeout`, `nice`, `nohup` and `time` are unwrapped: the segment is the wrapped command, with the wrapper recorded in `wrappers`. The wrapper's own options are parsed with hard-coded knowledge:

- `env`: leading `NAME=value` words go to `env`. Any option (`-i`, `-u`, `-S`, `--`, …) → `unsupported`.
- `timeout`: optional flags `-s SIG`/`--signal=SIG`, `-k DUR`/`--kill-after=DUR`, `--preserve-status`, `--foreground`, `-v`; then one duration word.
- `nice`: optional `-n N` / `-nN` / `--adjustment=N`.
- `nohup`, `time`: no options (`time -p` allowed).

Unrecognised wrapper options → `unsupported` (ask floor). A wrapper with no command following → treated as a plain segment named after the wrapper.

`sudo` is **not** a wrapper: it's an ordinary command name. The example configs ship an `ask` command rule for it.

### Floors

Floors are built-in and can't be configured away. Each floor contributes `ask` (a deny from any rule still wins). The floor constructs:

| Construct | Fires when |
|-----------|-----------|
| `parse_error` | the parser rejects the command |
| `unsupported` | any syntax-tree node kind not on the walker's allow-list, unsupported wrapper options, missing or non-string command field |
| `shell_reentry` | a segment `name` (basename) is a shell (`sh`, `bash`, `zsh`, `dash`, `ksh`, `fish`), runs code (`eval`, `source`, `.`, `exec`, `trap`), or changes how later commands run (`alias`, `unalias`, `set`, `setopt`, `unsetopt`, `shopt`, `emulate`, `zmodload`, `enable`, `disable`, `autoload`), whatever its arguments. `command` and `builtin` are ordinary commands. |
| `exec_tool` | `name` (basename) is `xargs`; or `name` is `find` and any arg is `-exec`, `-execdir`, `-ok`, `-okdir` or `-delete` |
| `dynamic_command` | the command name contains any expansion or substitution |
| `expansion` | any arg, env value or redirect target contains parameter expansion (`$x`, `${…}`, `$1`, …), arithmetic `$((…))`, command or process substitution, an unquoted glob (`*`, `?`, `[…]`) or brace expansion (`{a,b}`, `{1..3}`), or starts with an unquoted `=` (zsh equals expansion); also any of these in the body of a heredoc whose delimiter is unquoted (`<<EOF`; bodies of `<<'EOF'` / `<<"EOF"` are literal), and in a here-string (`<<<`) word. Quoted characters are literal. A leading `~` is not expansion. |
| `env_assign` | an assignment whose name doesn't match any `[shell] safe_env` regex: inline prefixes, `env NAME=…`, assignment-only segments (`PATH=./evil; cargo test`), and `NAME=value` arguments of `export`, `declare`, `typeset`, `local`, `readonly` |
| `cd` | `cd` / `pushd` to a path that isn't static and inside `{cwd}`; bare `cd`, `cd -`, `popd`, or any `cd` option |

Recursion into substitutions (including those in unquoted heredoc bodies) still happens when `expansion` fires, so a deny on an inner command wins.

#### Assignment-only segments

A segment with assignments and no command word (`FOO=1`) is neutral, like an in-project `cd`: it needs no command rule, and its names go through `env_assign`. `export`, `declare`, `typeset`, `local` and `readonly` are ordinary commands (they need a command rule to be allowed), but their `NAME=value` arguments are also checked by `env_assign`.

#### `cd` handling

`cd DIR` / `pushd DIR` with a static `DIR` resolving (via `paths::resolve`) inside `{cwd}` is neutral: it needs no command rule and doesn't count as a passthrough. It sets the **effective directory** used to resolve relative paths in **later segments** of the same command (for `paths_under` and `redirect_write`), in textual order. Subshell scoping is ignored: a `cd` inside `( … )` still affects later segments, which only errs towards checking against a deeper directory still under `{cwd}`. Command rules never see `cd` segments.

### Command rules

```toml
[[command_rule]]
decision = "allow"                 # required: allow | deny | ask
description = "cargo workflow"     # optional
reason = "..."                     # optional
match.text = { regex = '^cargo (build|test|check|clippy|fmt|run)\b' }
paths_under = ["{cwd}", "/tmp"]    # optional
```

- `match` is keyed by dotted field path into the **segment object** (`text`, `name`, `args`, `env.FOO`, …) and uses the existing matchers (`regex`, `not_regex`, `equals`, `glob`, `under`, `exists`), with the same semantics, `@pattern` references and errors. The docs and skills only teach `text` and `name`.
- No `tool` key: command rules only apply to the shell tool's segments.
- A command rule with no `match` and no `paths_under` is a config error (it would match every segment).
- `paths_under` (list of directories, with `~` and `{cwd}` like `under`): the rule matches only if every **path-like** value in the segment is under one of the directories. Path-like values:
  - args that contain `/`, or start with `.` or `~`;
  - for args of the form `--opt=value`, the `value` part, by the same test;
  - redirect targets other than fd dups (`2>&1`) and `/dev/null`.

  Relative paths resolve against the effective directory (see `cd`). Short flags with attached values (`-o/etc/x`) contain `/` and are checked as a path; they fail unless that odd path is inside, which errs towards not matching.

### Construct rules

Configurable structure checks. Each gives a decision when the construct is present.

```toml
[[construct_rule]]
decision = "ask"
construct = "background"
description = "backgrounded commands"

[[construct_rule]]
decision = "ask"
construct = "redirect_write"
description = "writes outside the project"
outside = ["{cwd}", "/tmp"]
```

| Construct | Present when | Extra keys |
|-----------|-------------|-----------|
| `redirect_write` | `>`, `>>`, `>|`, `&>`, `&>>`, `<>`, `N>` to a file (not fd dups; `/dev/null` excluded) | `outside`: only fires for targets **not** under these directories (resolved like `paths_under`); omitted = any write |
| `redirect_read` | `<`, `N<` from a file | — |
| `heredoc` | `<<`, `<<-`, `<<<` | — |
| `pipe` | a pipeline with more than one command | — |
| `background` | `&` | — |
| `subshell` | `( … )` | — |
| `substitution` | `$( … )`, backticks, `<( … )`, `>( … )` | — |

- Keys: `decision` (required), `construct` (required, one of the above; a floor name is a config error), `description`, `reason`, `outside` (only for `redirect_write`; elsewhere a config error).
- An `allow` construct rule is a config error: constructs can only raise to `ask` or `deny`.

### `[shell]`

```toml
[shell]
safe_env = ['^RUST_(LOG|BACKTRACE)$', '^NO_COLOR$', '^CI$', '^TERM$', '^LANG$', '^LC_']
```

- `safe_env`: regexes (or `@pattern` references) on assignment names. Default: empty, so every assignment asks.
- Unknown keys are errors.

### `[[rule]]` changes

`[[rule]]` is unchanged for all non-shell tools. For the shell tool, `[[rule]]` can **deny or ask** on the raw payload (e.g. `permission_mode`, `agent_id`, or a regex on the whole command), but **never allow**:

- An `allow` `[[rule]]` whose `tool` regex matches the agent's shell tool name, or which has no `tool`, is a **config error** ("shell tool allows must be command rules").

Pattern references (`[patterns]`) work in all three rule kinds.

## Evaluation

For a shell-tool call:

1. Evaluate `[[rule]]` entries against the raw payload (deny/ask only, by construction).
2. Parse the command. On failure: `parse_error`. Walk the tree with the allow-listed walker; collect segments (in textual order, including recursed ones), constructs and floors. Track the effective directory for `cd`.
3. For each non-`cd` segment, evaluate every command rule; the segment's decision is the highest of its matching rules (deny > ask > allow), or **none** if nothing matched.
4. Evaluate every construct rule against the collected constructs.
5. Combine:
   - **deny** if any `[[rule]]`, command rule or construct rule denies;
   - else **ask** if any of them asks, or any floor fired;
   - else **allow** if there is at least one segment and every non-`cd` segment's decision is allow;
   - else **passthrough** (some segment matched no command rule, or there were no segments).
6. The deciding item: the first deny or ask in this order: `[[rule]]` matches (file order), floors (textual order), segments (textual order, first matching rule), construct rules (file order). For allow, the first segment's first allowing rule.

A command that is only `cd` into the project (no other segments) is a passthrough.

Non-shell tools evaluate exactly as today.

### Reasons

Sent to the agent with every decision (Copilot and Claude show it for `deny` and `ask`):

- A rule: its `reason` if set, else the default `tool-gate-hook: <decision> by <kind> #<index> (<description>)`, where `<kind>` is `rule`, `command rule` or `construct rule`.
- A command rule also gets ` — in "<text>"` for the segment it matched, e.g. `tool-gate-hook: ask by command rule #6 (git push) — in "git push"`.
- A floor: `tool-gate-hook: ask — <description> (<detail>), in "<segment source>"`, leaving out the parts that don't apply. For example: `tool-gate-hook: ask — shell syntax that tool-gate-hook can't check (glob in docs/*.md), in "ls docs/*.md"`.
- Each floor has a fixed human-readable description (`parse_error`: "the command could not be parsed"; `unsupported`: "shell syntax that tool-gate-hook can't check"). The detail says what triggered it, e.g. `variable in $HOME`, `for loop`, `assignment FOO`.

### Rule indexes

Each rule kind is numbered separately, 1-based, in file order (`[[rule]]` #1…, `[[command_rule]]` #1…, `[[construct_rule]]` #1…). References in audit records and reasons carry the kind.

## Audit record

The existing record gains a `shell` object for shell-tool calls. The raw `payload` is kept (truncated as now). `decided_by` and `matches` entries carry a `kind` (`rule`, `command_rule`, `construct_rule`, `floor`); command-rule and floor matches also carry the 1-based `segment` they apply to, when there is one. Floors have no `index`, and their `description` is the construct name (`parse_error`, `unsupported`, …).

Top-level `matches` are in evaluation order: `[[rule]]` matches (file order), then floors (textual order), then command-rule matches segment by segment, then construct rules (file order).

```json
{
  "ts": "…", "agent": "claude", "config": "…",
  "decision": "ask",
  "decided_by": { "kind": "construct_rule", "index": 1, "description": "backgrounded commands" },
  "matches": [ { "kind": "construct_rule", "index": 1, "decision": "ask", "description": "backgrounded commands" } ],
  "shell": {
    "segments": [
      {
        "text": "cargo build", "name": "cargo", "args": ["build"], "env": {},
        "redirects": [ { "op": ">&", "fd": 2, "target": "1" } ], "wrappers": [],
        "source": "cargo build 2>&1",
        "matches": [ { "kind": "command_rule", "index": 3, "decision": "allow", "description": "cargo workflow" } ],
        "decision": "allow"
      },
      {
        "text": "./target/debug/thing --out /etc/x", "name": "./target/debug/thing",
        "args": ["--out", "/etc/x"], "env": {}, "redirects": [ { "op": ">", "target": "log.txt" } ],
        "wrappers": [], "source": "./target/debug/thing --out /etc/x > log.txt",
        "matches": [], "decision": null
      }
    ],
    "constructs": [
      { "construct": "background", "segment": 2, "floor": false },
      { "construct": "redirect_write", "segment": 2, "target": "log.txt", "floor": false }
    ]
  },
  "payload": { "…": "raw stdin JSON" },
  "duration_us": 512
}
```

- `segment` references are 1-based indexes into `segments`. `cd` segments appear with `"decision": "neutral"`.
- Floors appear in `constructs` with `"floor": true` and a `detail` string where useful (e.g. which variable).
- Strings in `shell` are truncated with `max_value_len` like the payload.
- `level = "matched"` records a shell call if any rule or floor matched.

## `explain` subcommand

```bash
tool-gate-hook explain --agent claude --config path.toml [--cwd DIR] 'COMMAND'
tool-gate-hook explain --agent claude --config path.toml --payload record-or-payload.json
```

- Builds a shell-tool payload for the agent (with `cwd` = `--cwd` or the current directory) from `COMMAND`, or reads `--payload` (a raw payload, or an audit record from which `payload` is taken), and runs it through `run()`.
- Prints the audit record that would be written, as pretty-printed JSON, regardless of `[audit] level`, with no truncation. Nothing is written to the audit file. It adds a `reason` after `decision`: the text the agent would be given (absent for passthrough). Audit records don't have it.
- An audit record whose payload was truncated by `max_value_len` could hide anything, so it is explained as `ask`, with a reason saying so and a warning on stderr; the segments and matches found in the truncated text are still shown.
- Exits 0 on any decision; non-zero only for a config that fails to load or unusable input (unlike `run`, which always exits 0).

## `validate` changes

- The summary includes counts per rule kind, construct rules by construct, and `safe_env`.
- New errors: allow `[[rule]]` that can match the shell tool; command rule with no conditions; allow construct rule; unknown construct; `outside` on a construct other than `redirect_write`.
- The existing unknown-field-path warning applies to `[[rule]]` only. For command rules, warn on match paths whose first component isn't a segment field.

## Parser

- Use **`brush-parser`** (MIT, 0.4.x; pin the version; needs Rust 1.88+). Consider the `serde` feature for debugging only; the segment object is our own type.
- The walker uses an explicit **allow-list** of node kinds. Anything else → `unsupported`.
- **Step 1 is a spike**: confirm brush-parser exposes enough word structure (quoted vs unquoted parts, expansions, heredocs, redirect fds, spans for `source`). If it doesn't, fall back to `tree-sitter-bash` with the same walker design.
- **Differential tests against real shells**: for a table of tricky static simple commands, compare our `name` + `args` with what **`zsh -f`** and **`/bin/bash`** produce, by running each command line with its command word replaced by a function that prints its arguments NUL-separated. Include `$'…'`, backslashes in and out of double quotes, line continuations, comments, adjacent quoted parts (`a"b"'c'`), empty strings, `#` mid-word, and words starting `=`. Cases where the shells disagree must be asserted to hit a floor instead. Each shell's tests are skipped if it isn't installed.

## Code changes (sketch)

- `src/shell.rs` (new): parse → segments, constructs, floors; wrappers; `cd` tracking; path-like detection; segment JSON.
- `src/agent.rs`: shell tool name and command field per agent.
- `src/config.rs`: `[shell]`, `[[command_rule]]`, `[[construct_rule]]`; new errors.
- `src/policy.rs`: evaluate command rules by running the existing field matchers against each segment's JSON value; `paths_under`; construct rules; combiner; rule kinds in `Decision` references.
- `src/auditing.rs`: `shell` object, `kind` on references, truncation of `shell`.
- `src/main.rs`: `explain` subcommand.
- `src/validate.rs`: summary and checks above.

Keep `run()` free of I/O beyond reading the config; `explain` reuses it.

## Examples, docs and skills

- Rewrite `examples/claude.toml`, `examples/copilot.toml`, `examples/mermaid-claude.toml`:
  - remove `shell_chain` and `parent_dir`;
  - shell allows become command rules on `text`; file-touching ones use `paths_under`;
  - `legacy_python` becomes a deny command rule on `name` (`'(^|/)(python[0-9.]*|pip[0-9]*|pipenv|virtualenv|pyenv)$'`);
  - an allow command rule for harmless builtins (`true`, `false`, `:`, `echo`, `printf`, `test`, `[`);
  - an ask command rule for `sudo`;
  - construct rules: ask on `background`, ask on `redirect_write` outside `{cwd}` and `/tmp`;
  - a `[shell] safe_env` list.
- `docs/configuration-guide.md`: a shell section (segments, command rules, `paths_under`, construct rules, floors, `[shell]`, evaluation, `explain`); update "Regexes see text, not shell syntax", the shell-chaining note and the worked examples; the audit section.
- `README.md`: brief mention and `explain`.
- `AGENTS.md`: update the code structure, the "out of scope" list (compound commands are now parsed), and remove the note about `legacy_python` firing on heredocs once the author's live config is migrated.
- Skills: `examples/skills/tool-gate-rules-claude/SKILL.md` and `examples/skills/tool-gate-rules-copilot/SKILL.md`, each using only its agent's tool names and field paths. Contents:
  1. When to use: the user wants a tool call auto-allowed, denied or always asked; or gives an audit entry to turn into a rule.
  2. Mental model: `[[rule]]` for tool payloads, `[[command_rule]]` per shell command, `[[construct_rule]]` for shell structure; deny > ask > allow > passthrough; floors can't be overridden; shell allows only via command rules.
  3. Workflow: find the config; read the audit entry or run `explain`; pick the segment or construct; write the narrowest rule; `validate`; `explain` the original command again and confirm the decision changed and nothing broadened.
  4. Recipes: allow a command family; allow file operations within a directory (`paths_under`); deny by command name; ask on writes outside the project; add a safe env var; deny a secrets path for file tools; subagent-only rules.
  5. Safety rules: narrowest rule; anchor with `^`; never broad allows (`'^.*'`, bare `rm`/`curl`/`git push`); never try to work around a floor; show the rule to the user, then edit the config.
  6. Reference: matchers, segment fields (`text`, `name`), construct and floor names, single-quoted TOML regexes.

## Testing

- Unit tests (small pure functions): word unquoting and re-quoting for `text`, path-like detection, wrapper option parsing, expansion detection.
- Differential tests against zsh and bash (above).
- Acceptance tests through `tool_gate_hook::run` with fixtures, covering at least:
  - `cargo build 2>&1 && cargo test` → allow; `foo && cargo test` → passthrough; `cargo test && rm -rf /` with a deny → deny.
  - heredoc / commit message containing a `pip install` line → not denied by `legacy_python`.
  - `cat <<EOF` with `$(curl x)` in the body → ask (and deny with a `curl` deny rule); the same with `<<'EOF'` → no floor.
  - `PATH=./evil; cargo test` and `export GIT_PAGER=x; git log` → ask; `alias ls=x` → ask; `=python3 x.py` → ask.
  - each floor → ask, and a deny beats a floor.
  - `env RUST_LOG=debug cargo test` → allow; `GIT_PAGER=x git log` → ask; `PATH=./evil cargo test` → ask.
  - `timeout 60 cargo test` → allow; `timeout --bogus 60 cargo test` → ask.
  - `find . -name '*.rs'` → allow with a find rule; `find . -exec rm {} \;` → ask; `ls | xargs rm` → ask.
  - `cd sub && rm x` with `paths_under = ["{cwd}"]` → allow; `cd /tmp && rm x` → ask; `cd sub && rm ../../x` → passthrough (fails `paths_under`).
  - `rm --out=/etc/x`-style and redirect targets checked by `paths_under`; `/dev/null` and `2>&1` ignored.
  - redirect outside `{cwd}` → ask via construct rule; inside → no construct match.
  - `echo $(curl evil)` with a deny on `curl` → deny.
  - config errors: allow `[[rule]]` on the shell tool (including no `tool`), allow construct rule, unknown construct, empty command rule.
  - Copilot `bash` payloads and `copilot_via_claude` payloads.
  - audit record `shell` shape; `explain` output for a command and for an audit record.
- Example configs and skills' recipe snippets are validated in tests.

## Implementation order

In vertical slices; see `plan.md`. Each step passes the quality gate (`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`).
