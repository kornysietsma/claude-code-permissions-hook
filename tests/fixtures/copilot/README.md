# Copilot fixtures

Payload shapes captured from real Copilot CLI sessions on 2026-10-04, with placeholder values (`cwd`, ids, timestamp). Copilot picks its tools per model: a GPT model used `apply_patch` (a string `toolArgs`) and `rg`, Haiku 4.5 used `create`. `edit` was never seen; see `docs/copilot-tool-inputs.md`. `rg_paths_list.json` is an `rg` call from 2026-10-08, where `paths` had become a list.

`../copilot_via_claude/` holds the Claude-format payloads Copilot sends to a hook registered in `.claude/settings.json`.
