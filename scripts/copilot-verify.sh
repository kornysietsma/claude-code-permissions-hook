#!/usr/bin/env bash
# Sets up, summarises and tears down the Copilot CLI verification repo.
# Walkthrough: docs/copilot-verification.md
#
#   scripts/copilot-verify.sh setup      create the test repo and register the user-level hook
#   scripts/copilot-verify.sh report     one line per audited call, per audit file
#   scripts/copilot-verify.sh push [-n]  copy audit logs and results.md to the home machine (-n: dry run)
#   scripts/copilot-verify.sh teardown   remove the user-level verification hook
#
# Environment overrides: TGH_BIN (binary), TGH_VERIFY_DIR (test repo), COPILOT_HOME,
# TGH_RESULTS_DEST (push destination, default personal-laptop:tgh-copilot-results).
set -euo pipefail

BIN="${TGH_BIN:-$HOME/.cargo/bin/tool-gate-hook}"
DIR="${TGH_VERIFY_DIR:-$HOME/tgh-copilot-verify}"
USER_HOOK="${COPILOT_HOME:-$HOME/.copilot}/hooks/tool-gate-hook-verify.json"

render() { sed -e "s|__DIR__|$DIR|g" -e "s|__BIN__|$BIN|g"; }

audit_only() {
  printf '[audit]\nfile = "%s/audit/%s.jsonl"\nlevel = "all"\nmax_value_len = 0\n' "$DIR" "$1" > "$2"
}

setup() {
  [ -x "$BIN" ] || { echo "no executable at $BIN (run: cargo install --path .)" >&2; exit 1; }
  mkdir -p "$DIR/audit" "$DIR/.github/hooks" "$DIR/.claude" "$(dirname "$USER_HOOK")"
  [ -d "$DIR/.git" ] || git -C "$DIR" init -q

  # User-level config: the only one that makes decisions.
  render > "$DIR/user.toml" <<'EOF'
[audit]
file = "__DIR__/audit/user.jsonl"
level = "all"
max_value_len = 0

# Shell commands are checked one command at a time, with [[command_rule]]
# Normally prompts, so an auto-run proves the allow worked
[[command_rule]]
decision = "allow"
description = "tgh-allow echo"
match.text = { regex = '^echo tgh-allow' }

[[command_rule]]
decision = "deny"
description = "tgh-deny"
reason = "tgh-verify: this command is denied on purpose"
match.text = { regex = 'tgh-deny' }

# Matches on the tool name only, so it works even if toolArgs field names are wrong
[[rule]]
decision = "ask"
tool = "glob"
description = "glob always asks"

# Normally runs without a prompt, so a prompt proves the ask worked
[[rule]]
decision = "ask"
tool = "view"
description = "view of tgh-ask files"
match."toolArgs.path" = { regex = 'tgh-ask' }

[[rule]]
decision = "deny"
tool = "view|create|edit"
description = "tgh-blocked paths"
reason = "tgh-verify: blocked paths are denied on purpose"
match."toolArgs.path" = { regex = 'tgh-blocked' }

# touch normally prompts (tgh-plain.txt is the control), so an auto-run proves the allow worked
[[command_rule]]
decision = "allow"
description = "touch tgh-allow-file"
match.text = { regex = '^touch tgh-allow-file\.txt$' }

# File changes arrive as apply_patch with a string toolArgs
[[rule]]
decision = "deny"
tool = "apply_patch"
description = "patches naming tgh-blocked"
reason = "tgh-verify: blocked paths are denied on purpose"
match."toolArgs" = { regex = '(?m)^\*\*\* (Add|Update|Delete) File: .*tgh-blocked' }
EOF

  # Repo-level and .claude cross-read configs: audit only, no rules, so they never decide anything.
  audit_only repo "$DIR/.github/hooks/tool-gate-hook.toml"
  audit_only claude-cross "$DIR/claude-cross.toml"

  render > "$USER_HOOK" <<'EOF'
{
  "version": 1,
  "hooks": {
    "preToolUse": [
      { "type": "command", "bash": "__BIN__ run --agent copilot --config __DIR__/user.toml", "timeoutSec": 10 }
    ]
  }
}
EOF

  # Repo-level: a cwd probe, plus the hook with a RELATIVE --config, as the README recommends.
  render > "$DIR/.github/hooks/tool-gate-hook.json" <<'EOF'
{
  "version": 1,
  "hooks": {
    "preToolUse": [
      { "type": "command", "bash": "pwd >> \"__DIR__/audit/repo-hook-cwd.txt\"", "timeoutSec": 10 },
      { "type": "command", "bash": "__BIN__ run --agent copilot --config .github/hooks/tool-gate-hook.toml", "timeoutSec": 10 }
    ]
  }
}
EOF

  # Claude-format hook in the repo, to see what Copilot sends to it (raw stdin is audited either way).
  render > "$DIR/.claude/settings.json" <<'EOF'
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "*",
        "hooks": [
          { "type": "command", "command": "__BIN__ run --agent claude --config __DIR__/claude-cross.toml" }
        ]
      }
    ]
  }
}
EOF

  printf '# tgh-copilot-verify\n\nScratch repo for verifying tool-gate-hook with Copilot CLI.\n' > "$DIR/README.md"
  printf 'hello\n' > "$DIR/tgh-ask.txt"
  printf 'harmless test text\n' > "$DIR/tgh-blocked.txt"

  "$BIN" validate --agent copilot --config "$DIR/user.toml" > /dev/null
  "$BIN" validate --agent copilot --config "$DIR/.github/hooks/tool-gate-hook.toml" > /dev/null
  "$BIN" validate --agent claude --config "$DIR/claude-cross.toml" > /dev/null
  echo "Set up $DIR; user-level hook registered at $USER_HOOK"
  echo "Next: cd $DIR && copilot"
}

report() {
  command -v jq > /dev/null || { echo "jq is required" >&2; exit 1; }
  for f in "$DIR"/audit/*.jsonl; do
    [ -e "$f" ] || continue
    echo "== $(basename "$f") ($(wc -l < "$f" | tr -d ' ') records)"
    jq -c '{
      ts: .ts[11:19],
      tool: (.payload | if type == "object" then (.toolName // .tool_name // "?") else "RAW: " + .[0:80] end),
      decision, by: .decided_by.index, error
    }' "$f"
  done
  if [ -e "$DIR/audit/repo-hook-cwd.txt" ]; then
    echo "== repo-hook-cwd.txt"
    sort "$DIR/audit/repo-hook-cwd.txt" | uniq -c
  fi
}

# Copies the audit logs and results.md to a new timestamped directory on the home machine.
# `push -n` shows what would be copied without copying anything.
push() {
  local flags=-av dest="${TGH_RESULTS_DEST:-personal-laptop:tgh-copilot-results}"
  [ "${1:-}" = "-n" ] && flags=-avn
  local target="$dest/$(date +%Y%m%d-%H%M%S)"
  if [ "$flags" = -av ]; then
    if [[ $target == *:* ]]; then ssh "${target%%:*}" mkdir -p "${target#*:}"; else mkdir -p "$target"; fi
  fi
  rsync $flags "$DIR/audit/" "$target/audit/"
  [ -e "$DIR/results.md" ] && rsync $flags "$DIR/results.md" "$target/"
  if [ "$flags" = -av ]; then echo "Results copied to $target"; else echo "Dry run only: nothing was copied"; fi
}

teardown() {
  rm -f "$USER_HOOK"
  echo "Removed $USER_HOOK. $DIR is left in place (delete it when finished)."
}

case "${1:-}" in
  setup) setup ;;
  report) report ;;
  push) shift; push "${1:-}" ;;
  teardown) teardown ;;
  *) echo "usage: $0 setup|report|push [-n]|teardown" >&2; exit 2 ;;
esac
