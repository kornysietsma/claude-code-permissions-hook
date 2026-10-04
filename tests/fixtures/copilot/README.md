# Copilot fixtures

Payload shapes captured from a real Copilot CLI session on 2026-10-04, with placeholder values (`cwd`, ids, timestamp). Copilot picks its tools per model, so `apply_patch` (a string `toolArgs`) and `rg` are what that session used instead of `create`, `edit` and `grep`; see `docs/copilot-tool-inputs.md`.

`../copilot_via_claude/` holds the Claude-format payloads Copilot sends to a hook registered in `.claude/settings.json`.
