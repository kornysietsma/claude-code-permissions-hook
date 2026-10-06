#!/usr/bin/env bash
# Extract the unique, untruncated shell commands from an audit log into a JSON-lines corpus
# (one JSON string per line). The output may contain private paths: keep it out of git.
#
#   scripts/shell-corpus.sh ~/.local/share/tool-gate-hook/claude.jsonl
set -euo pipefail

log="${1:?usage: $0 AUDIT_LOG [OUT]}"
out="${2:-tests/fixtures/shell/corpus.local.jsonl}"

jq -c '(.payload.tool_input.command // .payload.toolArgs.command)
       | select(type == "string" and (contains("…[truncated") | not))' "$log" \
  | sort -u > "$out"
echo "$(wc -l < "$out" | tr -d ' ') commands written to $out"
