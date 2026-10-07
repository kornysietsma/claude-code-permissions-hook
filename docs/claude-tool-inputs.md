# Claude Code hook payloads

What Claude Code sends to a `PreToolUse` hook on stdin, and so what `match."<path>"` rules can address. Rule field paths are dotted paths into this JSON, e.g. `tool_input.command` or `permission_mode`.

> **The audit log is the authoritative source.** Set `level = "all"` and `max_value_len = 0` in `[audit]` to record exactly what your Claude Code version sends (see [tests/README.md](../tests/README.md) for turning a record into a fixture). This document is a convenience snapshot, last compiled on 2026-10-03 from the Claude Code hooks documentation and the sources below; the top-level fields and the `Bash`, `Read`, `Write`, `Edit`, `Agent` and `SubagentHandback` payloads were checked against captures from Claude Code 2.1.289 on 2026-10-04; the other tools are from the sources below and unchecked.

**Contents**

- [Top-level fields](#top-level-fields)
- [Decision output](#decision-output)
- [Source attribution](#source-attribution)
- [File Operation Tools](#file-operation-tools): [Read](#read), [Write](#write), [Edit](#edit), [NotebookEdit](#notebookedit)
- [Search Tools](#search-tools): [Glob](#glob), [Grep](#grep)
- [Command Execution](#command-execution): [Bash](#bash)
- [Agent Tools](#agent-tools): [Agent (earlier versions: `Task`)](#agent-earlier-versions-task), [SubagentHandback](#subagenthandback)
- [Web Tools](#web-tools): [WebFetch](#webfetch), [WebSearch](#websearch)
- [Task Management](#task-management): [TodoWrite](#todowrite)
- [MCP Tools](#mcp-tools)

## Top-level fields

```json
{
  "session_id": "0d5f6c1e-test",
  "prompt_id": "550e8400-e29b-41d4-a716-446655440000",
  "transcript_path": "/Users/someone/.claude/projects/-Users-someone-prj-demo/0d5f6c1e-test.jsonl",
  "cwd": "/Users/someone/prj/demo",
  "permission_mode": "default",
  "effort": { "level": "high" },
  "hook_event_name": "PreToolUse",
  "tool_name": "Bash",
  "tool_input": { "command": "cargo test" },
  "tool_use_id": "toolu_01..."
}
```

| Field | Notes |
|-------|-------|
| `tool_name` | Matched by a rule's `tool`. Claude's tool names are capitalised (`Bash`, `Read`, ...). MCP tools are `mcp__<server>__<tool>`. |
| `cwd` | Required by this hook; also what `{cwd}` expands to in `under`. |
| `hook_event_name` | Always `PreToolUse` for this hook; any other value is treated as a payload for another purpose and passed through. |
| `tool_input` | A per-tool object, documented below. |
| `permission_mode`, `effort.level`, `agent_id`, `agent_type`, `session_id`, `prompt_id`, `tool_use_id`, `transcript_path`, `scratchpad_dir` | Available to rules, e.g. `match."permission_mode" = { equals = "plan" }`. `agent_id` and `agent_type` are only present when the call comes from a subagent. |

Verified against Claude Code 2.1.289 (2026-10-04):

- The subagent tool is `Agent` (it was `Task` in earlier versions; a rule can cover both with `tool = "Task|Agent"`).
- In auto mode, subagent completion fires a pseudo-tool, `SubagentHandback`, with `tool_input.message`. It carries `agent_id` and `agent_type`, and a rule with no `tool` matches it.
- `scratchpad_dir` is a top-level field (v2.1.257+), and `permission_mode` was `auto` in the captured session. The documented values are `default`, `plan`, `acceptEdits`, `auto`, `dontAsk` and `bypassPermissions`.
- `Glob` and `Grep` are absent by default on macOS, Linux and WSL, where file searches reach hooks as `Bash` calls (`find` and `grep`, embedded `bfs` and `ugrep`). They are part of the default tool set on Windows, so their sections below are kept but unverified.

The [official tools reference](https://code.claude.com/docs/en/tools-reference) lists the current tool names but not their `tool_input` fields. Tools it no longer lists, so not documented here: `MultiEdit`, `LS`, `BashOutput` and `KillShell` (the last two are replaced by `TaskOutput` and `TaskStop`).

## Decision output

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow|deny|ask",
    "permissionDecisionReason": "..."
  },
  "suppressOutput": true
}
```

No output means Claude continues its normal permission flow (Claude's `"defer"` decision means the same, so it isn't used). Claude treats exit code 2 as a block, so `tool-gate-hook run` always exits 0 and never signals a decision through the exit code.

## Source attribution

Anthropic does not publish a complete schema for `tool_input`. The per-tool tables below are compiled from:

| Source | Reliability | Notes |
|--------|-------------|-------|
| [Claude Code system prompt](https://gist.github.com/wong2/e0f34aac66caf890a332f7b6f9e2ba8f) | High | Extracted from actual sessions; schemas are embedded in the system prompt |
| [vtrivedy tools reference](https://www.vtrivedy.com/posts/claudecode-tools-reference) | Medium-High | Community-maintained, cross-referenced with the system prompt |
| [bgauryy implementation gist](https://gist.github.com/bgauryy/0cdb9aa337d01ae5bd0c803943aa36bd) | Medium | Reverse-engineered from behaviour |
| Direct observation | High | Hook inputs inspected in this project |

Schemas may change between Claude Code versions.

## File Operation Tools

Per-tool `tool_input` fields, grouped as Claude Code groups its tools.

### Read

Reads file contents. Can read text files, images, PDFs, and Jupyter notebooks.

```json
{
  "file_path": "/absolute/path/to/file.txt",
  "offset": 100,
  "limit": 50
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `file_path` | string | Yes | Absolute path to the file |
| `offset` | number | No | Line number to start reading from (1-indexed) |
| `limit` | number | No | Number of lines to read (default: 2000) |

### Write

Creates or overwrites a file.

```json
{
  "file_path": "/absolute/path/to/file.txt",
  "content": "file contents here"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `file_path` | string | Yes | Absolute path to the file |
| `content` | string | Yes | Complete file content to write |

### Edit

Performs exact string replacement in a file.

```json
{
  "file_path": "/absolute/path/to/file.txt",
  "old_string": "text to find",
  "new_string": "replacement text",
  "replace_all": false
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `file_path` | string | Yes | Absolute path to the file |
| `old_string` | string | Yes | Exact text to replace |
| `new_string` | string | Yes | Replacement text |
| `replace_all` | boolean | No | Replace all occurrences (default: false) |

### NotebookEdit

Edits Jupyter notebook cells.

```json
{
  "notebook_path": "/absolute/path/to/notebook.ipynb",
  "new_source": "print('hello')",
  "cell_id": "abc123",
  "cell_type": "code",
  "edit_mode": "replace"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `notebook_path` | string | Yes | Absolute path to .ipynb file |
| `new_source` | string | Yes | New cell content |
| `cell_id` | string | No | ID of cell to edit |
| `cell_type` | string | No | `"code"` or `"markdown"` |
| `edit_mode` | string | No | `"replace"`, `"insert"`, or `"delete"` |

## Search Tools

### Glob

Fast file pattern matching.

```json
{
  "pattern": "**/*.rs",
  "path": "/optional/search/directory"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `pattern` | string | Yes | Glob pattern (e.g., `"**/*.js"`, `"src/**/*.ts"`) |
| `path` | string | No | Directory to search in (default: cwd) |

### Grep

Content search using ripgrep.

```json
{
  "pattern": "fn\\s+main",
  "path": "/search/directory",
  "glob": "*.rs",
  "type": "rust",
  "output_mode": "content",
  "-A": 3,
  "-B": 3,
  "-C": 5,
  "-i": true,
  "-n": true,
  "multiline": false,
  "head_limit": 100
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `pattern` | string | Yes | Regular expression to search for |
| `path` | string | No | File or directory to search |
| `glob` | string | No | Filter files by glob pattern |
| `type` | string | No | Filter by file type (`"js"`, `"py"`, `"rust"`, etc.) |
| `output_mode` | string | No | `"content"`, `"files_with_matches"`, or `"count"` |
| `-A` | number | No | Lines to show after match |
| `-B` | number | No | Lines to show before match |
| `-C` | number | No | Lines to show before and after match |
| `-i` | boolean | No | Case insensitive search |
| `-n` | boolean | No | Show line numbers (default: true) |
| `multiline` | boolean | No | Enable multiline matching |
| `head_limit` | number | No | Limit output to first N results |

## Command Execution

### Bash

Executes shell commands.

```json
{
  "command": "cargo build --release",
  "description": "Build release binary",
  "timeout": 120000,
  "run_in_background": false
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `command` | string | Yes | Shell command to execute |
| `description` | string | No | 5-10 word description |
| `timeout` | number | No | Timeout in milliseconds (default: 120000, max: 600000) |
| `run_in_background` | boolean | No | Run asynchronously |

## Agent Tools

### Agent (earlier versions: `Task`)

Launches a subagent for complex tasks.

```json
{
  "description": "Search for auth code",
  "prompt": "Find all authentication-related code in the codebase",
  "subagent_type": "Explore",
  "model": "haiku",
  "resume": "agent-id-123"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `description` | string | Yes | Short 3-5 word task description |
| `prompt` | string | Yes | Detailed task instructions |
| `subagent_type` | string | Yes | Agent type (see below) |
| `model` | string | No | `"sonnet"`, `"opus"`, or `"haiku"` |
| `resume` | string | No | Agent ID to resume from |

**Subagent types** (may vary by Claude Code version):
- `general-purpose` - Full tool access for complex tasks
- `Explore` - Fast codebase exploration (Glob, Grep, Read, Bash)
- `Plan` - Software architecture planning
- `statusline-setup` - Configure status line (Read, Edit)

### SubagentHandback

Delivers a subagent's final report to the conversation that receives it. Per the tools reference it is only provided in auto mode (Claude Code v2.1.271+), to locally run subagents other than forks.

```json
{ "message": "I appended the line; the file is otherwise unchanged." }
```

| Field | Type | Description |
|-------|------|-------------|
| `message` | string | The subagent's final message |

The top-level payload also carries `agent_id` and `agent_type`.

## Web Tools

### WebFetch

Fetches and processes web content.

```json
{
  "url": "https://example.com/docs",
  "prompt": "Extract the API endpoints from this page"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `url` | string | Yes | Fully-formed URL (HTTP upgraded to HTTPS) |
| `prompt` | string | Yes | What information to extract |

### WebSearch

Searches the web.

```json
{
  "query": "rust async tutorial 2025",
  "allowed_domains": ["docs.rs", "rust-lang.org"],
  "blocked_domains": ["pinterest.com"]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `query` | string | Yes | Search query (min 2 characters) |
| `allowed_domains` | array | No | Only include these domains |
| `blocked_domains` | array | No | Exclude these domains |

## Task Management

The task list is now handled by `TaskCreate`, `TaskGet`, `TaskList` and `TaskUpdate` on current models (their `tool_input` fields are not captured here). Despite the names they are unrelated to the subagent tool `Task`, which is now `Agent`. `TodoWrite` is disabled by default in favour of them.

### TodoWrite

Manages task list (older sessions, or with `CLAUDE_CODE_ENABLE_TASKS=0`).

```json
{
  "todos": [
    {
      "content": "Implement feature X",
      "activeForm": "Implementing feature X",
      "status": "in_progress"
    }
  ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `todos` | array | Yes | Array of todo items |
| `todos[].content` | string | Yes | Task description (imperative form) |
| `todos[].activeForm` | string | Yes | Task description (present continuous) |
| `todos[].status` | string | Yes | `"pending"`, `"in_progress"`, or `"completed"` |

## MCP Tools

MCP (Model Context Protocol) tools have dynamic schemas defined by their servers. They follow the naming pattern `mcp__<server>__<tool>`. To match MCP tools in hooks, use regex patterns like:

```
mcp__.*           # All MCP tools
mcp__github__.*   # All GitHub MCP tools
```

