---
name: tool-gate-rules-claude
description: Write or change tool-gate-hook rules for Claude Code, so a tool call or shell command is auto-allowed, denied or always asked. Use when the user wants a command or tool call to stop prompting, wants something blocked or always confirmed, or gives a tool-gate-hook audit log entry (or a tool-gate-hook reason) to turn into a rule.
---

# Writing tool-gate-hook rules for Claude Code

`tool-gate-hook` is a `PreToolUse` hook that decides tool calls from rules in a TOML file. This skill is for **Claude Code** configs (`--agent claude`). Tool names are capitalised (`Bash`, `Read`, `Write`, `Edit`, `Glob`, `Grep`, `Agent`) and arguments are under `tool_input`.

## When to use

- The user wants a tool call or shell command auto-allowed ("stop asking me about `npm run lint`").
- The user wants something denied, or always confirmed.
- The user pastes an audit log entry, or a reason starting `tool-gate-hook:`, and wants a rule from it.

## How it decides

- `[[rule]]` matches the raw payload of **any** tool call (`tool_input.file_path`, `agent_id`, …). For `Bash` it can only **deny or ask**, never allow.
- `[[command_rule]]` matches **each command inside a `Bash` call** (a segment). `cargo build 2>&1 && cargo test` is two segments. This is the only way to allow shell commands.
- `[[construct_rule]]` matches shell structure: `redirect_write`, `redirect_read`, `heredoc`, `pipe`, `background`, `subshell`, `substitution`, or a floor name (below) to ask whenever that floor fires. Ask or deny only.
- Every rule is checked. **deny > ask > allow > passthrough** (passthrough = no rule decided, Claude asks as usual).
- A `Bash` call is allowed only if **every** segment is allowed by a command rule and nothing asks or denies.
- **Floors** are built-in checks that turn an allow into an ask: variables (`$X`), globs (`*.md`), command substitution, `for`/`if`/`while`, `eval`, `bash -c`, `xargs`, `find -exec`, `cd` outside the project, assignments not in `[shell] safe_env`, and anything that won't parse. **No rule can turn a floor into an allow.** A deny still wins over a floor.

## Workflow

1. **Find the config.** Usually `~/.config/tool-gate-hook/claude.toml`. Otherwise look at the hook command in `~/.claude/settings.json` or `.claude/settings.json` for a `--config` path.
2. **See what happens now.** For a command:
   ```bash
   tool-gate-hook explain --agent claude --config CONFIG --cwd PROJECT_DIR 'THE COMMAND'
   ```
   For an audit log entry, save the JSON line to a file and run `tool-gate-hook explain --agent claude --config CONFIG --payload FILE` (or pipe it in with `--payload -`).
   The output has `decision`, `reason`, and `shell.segments` (each with `text`, `name` and `decision`) and `shell.constructs` (with `floor: true` for floors).
3. **Pick what to change.**
   - A segment with `"decision": null` matched no command rule: write a command rule for it.
   - A construct rule asked: decide if the user really wants that changed. Usually they don't.
   - A floor (`"floor": true`): **don't write a rule**. Tell the user why it asks and how to write the command so it can be checked (spell out paths instead of `$VAR` or globs; avoid `for` loops; use `echo ---`, not `echo ====`).
4. **Write the narrowest rule** that covers the request (see Recipes and Safety rules).
5. **Show the rule to the user** before editing the config, then add it.
6. **Validate:** `tool-gate-hook validate --agent claude --config CONFIG`. Fix any error or warning.
7. **Check again:** `explain` the original command; the decision must have changed for the right reason. Also `explain` a riskier variant (`… && rm -rf x`, the command with `/etc/x` as an argument) and confirm it is **not** allowed.

## Recipes

Each recipe is a complete, valid config fragment. Copy only the rules you need.

Allow a command family (match `text`, the command and its args with quotes removed; anchor with `^` and end with `\b` or `$`):

```toml recipe
[[command_rule]]
decision = "allow"
description = "npm lint and test"
match.text = { regex = '^npm run (lint|test)$' }
```

Allow a command only on files inside some directories (`paths_under` checks every path-like argument and redirect target):

```toml recipe
[[command_rule]]
decision = "allow"
description = "rm inside the project"
match.name = { equals = "rm" }
paths_under = ["{cwd}"]
```

Deny a command by name, wherever it runs (in a chain, a pipe, `$( )`), with a reason that tells the model what to do instead:

```toml recipe
[[command_rule]]
decision = "deny"
description = "legacy Python tooling"
reason = "Use uv: uv run script.py, uv add pkg"
match.name = { regex = '(^|/)(python[0-9.]*|pip[0-9]*)$' }
```

Always ask for a command, even if another rule allows it:

```toml recipe
[[command_rule]]
decision = "ask"
description = "git push"
match.text = { regex = '^git push\b' }
```

Ask before writing files outside the project, or backgrounding a command:

```toml recipe
[[construct_rule]]
decision = "ask"
construct = "redirect_write"
description = "writes outside the project"
outside = ["{cwd}", "/tmp"]

[[construct_rule]]
decision = "ask"
construct = "background"
description = "backgrounded commands"
```

Let a variable be set without asking (`RUST_LOG=debug cargo test`). Merge into the existing `[shell]` table if there is one; a file can have only one:

```toml recipe
[shell]
safe_env = ['^RUST_(LOG|BACKTRACE)$', '^NO_COLOR$']
```

Allow reads inside the project, and deny secrets files for file tools:

```toml recipe
[[rule]]
decision = "allow"
tool = "Read"
description = "reads inside the project"
match."tool_input.file_path" = { under = ["{cwd}"] }

[[rule]]
decision = "deny"
tool = "Read|Write|Edit"
description = "secrets files"
reason = "Secrets files are off limits"
match."tool_input.file_path" = { regex = '\.(env|pem|secret)$' }
```

Rules only for subagents (`agent_id` is present only in subagent calls):

```toml recipe
[[rule]]
decision = "ask"
tool = "Bash|Write|Edit"
description = "subagents ask before shell commands and edits"
match."agent_id" = { exists = true }
```

## Safety rules

- **Narrowest rule that works.** Allow `^npm run lint$`, not `^npm\b`.
- **Anchor** command regexes with `^`, and end them with `\b`, `$` or a space.
- **Never write broad allows**: no `regex = '^.*'` or `''`, no bare `rm`, `curl`, `git push`, `sudo`, `ssh`, interpreters (`node`, `ruby`, `uv run`) or package installs without tight arguments. A command that can run arbitrary code is not "safe" because its name is.
- File-changing commands (`rm`, `mv`, `cp`, `curl -o`) need `paths_under`.
- **Never work around a floor** (no rewriting the user's command into `bash -c`, no allow rules on `eval`, `sh` or `xargs`). Explain it instead.
- Before writing `under`, `paths_under` or `outside` for a directory, check it for symlinks (`find DIR -type l -exec ls -l {} +`). Paths resolve through symlinks, so content linked in from elsewhere isn't "under" the directory; list the link targets too, and tell the user why. A listed directory that is itself a symlink is fine.
- Don't add variables like `PATH`, `GIT_PAGER`, `LD_PRELOAD` or `EDITOR` to `safe_env`: they change what later commands run.
- `[[rule]]` can't allow `Bash`; that's a config error. Use `[[command_rule]]`.
- Prefer a `reason` on deny rules that tells the model what to do instead.
- Show the rule to the user before editing their config.

## Reference

**Matchers** (all given must pass): `regex` (string or list, any matches), `not_regex` (none may match), `equals`, `glob`, `under` (list of directories; `~` and `{cwd}` work; resolves symlinks and `..`, so symlinks inside a directory that point elsewhere aren't under it), `exists` (true/false).

**Segment fields for command rules:** use `text` (command and args, quotes removed, re-quoted only where needed: `git commit -m 'fix bug'`) or `name` (the command word, e.g. `git`, `./scripts/run.sh`). `env`, `timeout`, `nice`, `nohup` and `time` are unwrapped, so `timeout 60 cargo test` has the text `cargo test`.

**Path-like values for `paths_under`:** args containing `/` or starting with `.` or `~`, the value of `--opt=value`, and redirect targets (not `2>&1` or `/dev/null`). `-o/etc/x` style args make the rule not match. After `cd sub`, paths are checked from both the project root and `sub`, so `cd sub && rm ../x` never passes; use paths from the project root.

**Construct rule names:** `redirect_write` (with optional `outside`), `redirect_read`, `heredoc`, `pipe`, `background`, `subshell`, `substitution`.

**Floor names** (seen in `explain` output; ask instead of allowing; not configurable): `parse_error`, `unsupported`, `expansion`, `dynamic_command`, `env_assign`, `shell_reentry`, `exec_tool`, `cd`.

**TOML:** write regexes in single quotes (`'^cargo\b'`), so backslashes need no doubling. Quote dotted payload paths: `match."tool_input.file_path"`. Rules of each kind are numbered separately from 1 in file order (`command rule #3`); new rules can go anywhere.

**Full reference:** `docs/configuration-guide.md` in the tool-gate-hook repository.
