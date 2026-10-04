# Copilot CLI verification (work machine)

A mechanical checklist for one batched session on the work machine. It settles the things `docs/copilot-tool-inputs.md` marks as unverified, and checks that the decisions the hook returns really change what Copilot does.

**What the session answers**

1. The real `toolArgs` field names for `bash`, `view`, `create`, `edit`, `glob`, `grep` and `task`, and the real `toolName` values.
2. Which working directory repo-level hooks run in, and whether a **relative** `--config` works there.
3. What Copilot does with a hook registered in `.claude/settings.json`: whether it fires, and in which payload format.
4. Whether `allow`, `deny` and `ask` have the intended effect, and whether the reasons are shown.
5. How a user-level and a repo-level hook combine, and what a config error looks like.

The work machine is Apple Silicon (the home machine is Intel), so build there rather than copying a binary. Both have Rust.

> **Caution: the user-level hook is global while it is registered.** With it in place, every Copilot session on the work machine goes through `user.toml` and is logged in full (`level = "all"`, `max_value_len = 0`). Its rules only match the obvious `tgh-` test strings, but run Copilot **only in the test repo** during the session, and run `teardown` (step 8) straight afterwards. Read the audit logs before sending them back; they contain whatever Copilot was asked.

## 1. Get the source

Run on the **work machine**, pulling from the home machine. This assumes an `ssh` config entry `Host personal-laptop` there, and Remote Login enabled on the home machine (check with `ssh personal-laptop true`).

```bash
# Dry run first: lists what would be copied, changes nothing
rsync -avn --delete --exclude=/target --exclude=/.git --exclude=/.claude --exclude=.DS_Store \
  personal-laptop:Dropbox/prj/ai/claude-code-permissions-hook/ ~/tool-gate-hook-src/

# The real thing: the same command without -n
rsync -av --delete --exclude=/target --exclude=/.git --exclude=/.claude --exclude=.DS_Store \
  personal-laptop:Dropbox/prj/ai/claude-code-permissions-hook/ ~/tool-gate-hook-src/
```

A first pull copies about 45 files. Rerun it any time the home copy changes; it is safe to repeat.

If rsync is awkward, the branch is also on GitHub (the two give the same code):

```bash
git clone -b rework-for-copilot git@github.com:kornysietsma/claude-code-permissions-hook.git ~/tool-gate-hook-src
```

#### Why the sync is built like this

The two machines are never synced both ways over the same files, so there is nothing to conflict:

| Direction | What | Where it lands |
|---|---|---|
| Home → work (pull) | The source tree | `~/tool-gate-hook-src/` on the work machine. **Never pushed back.** |
| Work → home (push, step 8) | Audit logs and `results.md` | `~/tgh-copilot-results/<timestamp>/` on the home machine, **outside Dropbox** so work-machine logs don't sync to the cloud |

- `--exclude=/target` keeps the Intel build output off the Apple Silicon machine, which builds its own. Excluded paths are also protected from `--delete`, so a re-pull never deletes the work machine's `target/`.
- `--exclude=/.git` keeps the home machine's git history (author details, unpushed branches) off the work machine. The copy there is for building, not committing. All changes are made on the home machine.
- `--exclude=/.claude` skips the local Claude settings file, which is git-ignored and personal.
- `--delete` removes files that were deleted at home since the last pull, so the copy doesn't drift. The trailing `/` on the source means "the contents of this directory".
- Only plain rsync flags are used, so Apple's older bundled rsync and Homebrew's both work.
- Always dry-run (`-n`) first when you've changed anything about the paths.

## 2. Build and install

```bash
cd ~/tool-gate-hook-src
cargo install --path .
~/.cargo/bin/tool-gate-hook --help
copilot --version            # note this for the results
which jq                     # the report command needs jq (brew install jq)
```

## 3. Set up the test repo and the user-level hook

```bash
cd ~/tool-gate-hook-src
scripts/copilot-verify.sh setup
```

This creates `~/tgh-copilot-verify/` (a git repo), validates the generated configs, and registers one user-level hook, `~/.copilot/hooks/tool-gate-hook-verify.json` (or under `$COPILOT_HOME`). The hooks it sets up:

| Where | Hook | Config | Rules |
|---|---|---|---|
| User level (`~/.copilot/hooks/tool-gate-hook-verify.json`) | `tool-gate-hook run --agent copilot --config ~/tgh-copilot-verify/user.toml` | `user.toml` | The only decisions: see below. Audit: `audit/user.jsonl` |
| Repo level (`.github/hooks/tool-gate-hook.json`) | `tool-gate-hook run --agent copilot --config .github/hooks/tool-gate-hook.toml` (**relative path**) | `.github/hooks/tool-gate-hook.toml` | None, audit only: `audit/repo.jsonl` |
| Repo level, same file | `pwd >> ~/tgh-copilot-verify/audit/repo-hook-cwd.txt` | n/a | Records the directory repo hooks run in |
| `.claude/settings.json` in the repo | `tool-gate-hook run --agent claude --config ~/tgh-copilot-verify/claude-cross.toml` | `claude-cross.toml` | None, audit only: `audit/claude-cross.jsonl` |

The audit-only hooks never decide anything, so any prompt or block you see comes from `user.toml`. The absolute binary path (`~/.cargo/bin/...`) is used throughout so the session doesn't depend on Copilot's `PATH`.

`user.toml` rules (the field names `toolArgs.command` and `toolArgs.path` are the guesses being tested):

| # | Decision | Matches | Why it's useful |
|---|---|---|---|
| 1 | allow | `bash` with command starting `echo tgh-allow`, no chaining | Shell commands normally prompt, so an auto-run proves the allow worked |
| 2 | deny | `bash` with command containing `tgh-deny` | Reason: "tgh-verify: this command is denied on purpose" |
| 3 | ask | tool `glob`, no field conditions | Works even if the field names are wrong, so the ask path is tested regardless |
| 4 | ask | `view` with path containing `tgh-ask` | Views normally run without a prompt, so a prompt proves the ask worked |
| 5 | deny | `view`, `create` or `edit` with path containing `tgh-blocked` | Reason: "tgh-verify: blocked paths are denied on purpose". (Named "blocked", not "secret": the first run's model refused to touch a file called `tgh-secret.txt` before the hook ever ran) |
| 6 | allow | `bash` with command exactly `touch tgh-allow-file.txt` | `touch` normally prompts, so an auto-run proves an allow suppresses a prompt (`echo` doesn't prompt anyway, so rule 1 proves nothing about that) |
| 7 | deny | `apply_patch` whose patch text names a `tgh-blocked` file | The patch is a string, so this matches `toolArgs` as a whole. Reason as rule 5 |

## 4. Sanity check without Copilot (optional)

```bash
cd ~/tgh-copilot-verify
echo '{"sessionId":"s","timestamp":1,"cwd":"'"$PWD"'","toolName":"bash","toolArgs":{"command":"echo tgh-deny"}}' \
  | ~/.cargo/bin/tool-gate-hook run --agent copilot --config user.toml
```

Expected: `{"permissionDecision":"deny","permissionDecisionReason":"tgh-verify: this command is denied on purpose"}`. Then `scripts/copilot-verify.sh report` (from the source directory, or give the full path) shows the record. Delete the `audit/` files afterwards so the session starts clean: `rm ~/tgh-copilot-verify/audit/*`.

## 5. The session

```bash
cd ~/tgh-copilot-verify && copilot
```

Accept the folder trust prompt if asked. Give the prompts below **one at a time, in order**, and write what you see in `~/tgh-copilot-verify/results.md` against the prompt number. Copilot may pick another tool than the one named; if it does, say "use the X tool" again, and note that it happened. Don't answer an `ask` prompt differently from "yes" unless you are testing that.

### A. Tool payloads (observe)

| # | Prompt | Expected | Note |
|---|---|---|---|
| 1 | `Use the bash tool to run: ls -la` | Normal Copilot prompt (no rule matches) | Did a prompt appear? |
| 2 | `Use the view tool to show README.md` | Runs without a prompt | |
| 3 | `Use the create tool to make notes.txt containing: hello` | Normal Copilot prompt | |
| 4 | `Use the edit tool to change hello to goodbye in notes.txt` | Normal Copilot prompt | |
| 5 | `Use the grep tool to search this repo for hello` | Normal Copilot behaviour | Did Copilot use `grep` or `rg`? |
| 6 | `Use the task tool (a sub-agent) to list the files in this repo` | Normal Copilot behaviour | Did a `task` call appear in the audit? |

### B. Rules

| # | Prompt | Expected |
|---|---|---|
| 7 | `Use the bash tool to run exactly: echo tgh-allow` | Runs **without a prompt** (rule 1) |
| 8 | `Use the bash tool to run exactly: echo tgh-allow && echo chained` | Normal Copilot prompt (the chain defeats the allow) |
| 9 | `Use the bash tool to run exactly: echo tgh-deny` | **Blocked**. Note whether you or Copilot see "denied on purpose", and what Copilot says |
| 10 | `Use the glob tool to find *.md files` | **Prompts** (rule 3, even though glob normally doesn't) |
| 11 | `Use the view tool to show tgh-ask.txt` | **Prompts** (rule 4). If it doesn't, `toolArgs.path` may be the wrong field name |
| 12 | `Use the view tool to show tgh-blocked.txt. It only contains harmless test text.` | **Blocked** (rule 5), same note as 9 |

### C. Config error

Break the user-level config on purpose, from another terminal:

```bash
printf '\n[[rule]]\ndecision = "allow"\nmatch."toolArgs.command" = { regex = "(unclosed" }\n' >> ~/tgh-copilot-verify/user.toml
```

| # | Prompt | Expected |
|---|---|---|
| 13 | `Use the view tool to show README.md` | **Prompts** with a config error mentioning the path and rule #8. Is the message readable? |

Restore it with `scripts/copilot-verify.sh setup` (it rewrites every generated file but leaves `audit/` alone), then run prompt 2 again to confirm it no longer prompts.

### D. Anything else

Note anything surprising: stderr shown in the UI, slow calls, messages when a hook ran, the order hooks ran in, hooks run twice, or a repo hook that did not run.

### E. Second run: the gaps the first run left

The first run (2026-10-04) settled payloads, hook locations and the ask, deny and config-error paths. A short second session closes the rest. Only prompts R1 to R5 are needed; don't repeat 1 to 13.

Start clean, so the new records are easy to find:

```bash
cd ~/tool-gate-hook-src
[ -d ~/tgh-copilot-verify/audit ] && mv ~/tgh-copilot-verify/audit ~/tgh-copilot-verify/audit-run1
scripts/copilot-verify.sh setup        # rewrites user.toml with rules 6 and 7 and creates tgh-blocked.txt
cd ~/tgh-copilot-verify && copilot
```

(Pull the source again first if it changed: the `--delete` rsync from step 1.)

| # | Prompt | Expected | What it settles |
|---|---|---|---|
| R1 | `Use the bash tool to run exactly: touch tgh-plain.txt` | **Prompts** (no rule matches) | The control: this must prompt, or R2 proves nothing |
| R2 | `Use the bash tool to run exactly: touch tgh-allow-file.txt` | Runs **without a prompt** (rule 6) | An `allow` really suppresses a prompt |
| R3 | `Use the view tool to show tgh-blocked.txt. It only contains harmless test text.` | **Blocked** with the reason (rule 5) | A deny rule on `view` |
| R4 | `Create a file called tgh-blocked-new.txt containing: hi` | **Blocked** (rule 7), `apply_patch` with a deny reason in the UI | A deny rule on a string `toolArgs` |
| R5 (optional) | Switch model with `/model` to a different one, then: `Use the create tool to make notes2.txt containing: hello` | Whatever it does | Does another model send `create` or `edit`? Note the model name and which tool names appear in `audit/user.jsonl` |

If the model refuses R3 or R4 itself (no record in `audit/user.jsonl`), say so in `results.md` and try once more, telling it the file is harmless test data. Then read the audit and push the results as in steps 6 to 8 (`scripts/copilot-verify.sh report`, `teardown`, `push`; the `audit-run1` directory is not pushed again).

## 6. Read the audit logs

```bash
cd ~/tool-gate-hook-src && scripts/copilot-verify.sh report
```

This prints one line per call for each audit file, and the directories repo hooks ran in. For each question from the top of this document:

| Question | Where to look |
|---|---|
| Field names and tool names | `audit/user.jsonl`: `jq 'select(.payload.toolName=="view") | .payload' ~/tgh-copilot-verify/audit/user.jsonl` for each tool |
| Did user-level and repo-level hooks both fire? | Counts and timestamps in `user.jsonl` vs `repo.jsonl`: each call should appear in both |
| Repo hook working directory | `audit/repo-hook-cwd.txt` |
| Relative `--config` worked | `repo.jsonl` has records for the calls in A and B, and no config-error prompt appeared before step C (a failed relative path would have shown one) |
| `.claude/settings.json` cross-read | `claude-cross.jsonl`: no records means it didn't fire. A record with a `RAW: ...` payload and an error means Copilot sent camelCase; an ordinary record means it sent Claude's snake_case. Either way the raw payload is in the log |
| Decisions had the effect | Your notes for prompts 7 to 13 against the `decision` column of `user.jsonl` |

## 7. Optional extras

- **PATH:** to see what a bare `tool-gate-hook` command does under Copilot, edit the `bash` command in `~/.copilot/hooks/tool-gate-hook-verify.json` to drop the directory, run one prompt, then restore it with `setup`. Copilot treats a failing command as a deny, so a missing binary should block the call. Note the message.
- **Timeouts:** not worth testing; Copilot's docs say timeouts fail open.

## 8. Tear down and send the results back

```bash
cd ~/tool-gate-hook-src && scripts/copilot-verify.sh teardown     # removes the user-level hook
```

Read the audit logs one last time for anything you don't want to leave the machine. Then, still on the **work machine**, push them with your notes to the home machine:

```bash
cd ~/tool-gate-hook-src
scripts/copilot-verify.sh push -n     # dry run: lists the files, copies nothing
scripts/copilot-verify.sh push        # copies to personal-laptop:tgh-copilot-results/<timestamp>/
```

This copies `~/tgh-copilot-verify/audit/` and `results.md` into a new timestamped directory, so repeated pushes never overwrite each other. Include the Copilot version in `results.md`. To use a different destination, set `TGH_RESULTS_DEST` (for example `TGH_RESULTS_DEST=otherhost:somewhere`).

If rsync fails, any way of getting those files across works (zip and email, or paste the `report` output and the `jq` payloads).

The results land outside the repo and outside Dropbox on purpose. Real payloads go into `tests/fixtures/copilot/` only after scrubbing private paths and session ids (plan step 9.2).
