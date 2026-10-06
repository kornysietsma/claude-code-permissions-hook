use crate::policy::Decision;
use anyhow::{Result, anyhow, bail};
use clap::ValueEnum;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Agent {
    Claude,
    Copilot,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Copilot => "copilot",
        }
    }

    /// The tool that runs shell commands, and where its payload holds the command
    pub fn shell_tool(self) -> ShellTool {
        match self {
            Agent::Claude => ShellTool {
                name: "Bash",
                command_path: "tool_input.command",
            },
            Agent::Copilot => ShellTool {
                name: "bash",
                command_path: "toolArgs.command",
            },
        }
    }

    /// A minimal shell tool payload running `command` in `cwd`
    pub fn shell_payload(self, command: &str, cwd: &Path) -> Value {
        match self {
            Agent::Claude => json!({
                "hook_event_name": "PreToolUse",
                "cwd": cwd,
                "tool_name": self.shell_tool().name,
                "tool_input": { "command": command },
            }),
            Agent::Copilot => json!({
                "cwd": cwd,
                "toolName": self.shell_tool().name,
                "toolArgs": { "command": command },
            }),
        }
    }

    pub fn default_config_path(self, home: &Path) -> PathBuf {
        home.join(".config")
            .join("tool-gate-hook")
            .join(format!("{}.toml", self.name()))
    }

    /// Extracts what the engine needs, or explains why the payload isn't from this agent
    pub fn parse(self, payload: &Value) -> Result<ToolCall> {
        match self {
            Agent::Claude => {
                if let Some(event) = payload.get("hook_event_name")
                    && event != "PreToolUse"
                {
                    bail!("not a PreToolUse payload (hook_event_name = {event})");
                }
                self.tool_call(payload, "tool_name")
            }
            Agent::Copilot => self.tool_call(payload, "toolName"),
        }
    }

    fn tool_call(self, payload: &Value, tool_key: &str) -> Result<ToolCall> {
        Ok(ToolCall {
            tool_name: self.string_field(payload, tool_key)?,
            cwd: PathBuf::from(self.string_field(payload, "cwd")?),
        })
    }

    fn string_field(self, payload: &Value, key: &str) -> Result<String> {
        payload
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("no string {key}; not a {} payload", self.name()))
    }

    /// Top-level payload keys documented for this agent
    pub fn top_level_keys(self) -> &'static [&'static str] {
        match self {
            Agent::Claude => &[
                "session_id",
                "prompt_id",
                "transcript_path",
                "cwd",
                "scratchpad_dir",
                "permission_mode",
                "effort",
                "hook_event_name",
                "agent_id",
                "agent_type",
                "tool_name",
                "tool_input",
                "tool_use_id",
            ],
            Agent::Copilot => &["sessionId", "timestamp", "cwd", "toolName", "toolArgs"],
        }
    }

    pub fn render(self, decision: Decision, reason: &str) -> Value {
        match self {
            Agent::Claude => json!({
                "hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": decision.as_str(),
                    "permissionDecisionReason": reason,
                },
                "suppressOutput": true,
            }),
            Agent::Copilot => json!({
                "permissionDecision": decision.as_str(),
                "permissionDecisionReason": reason,
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellTool {
    pub name: &'static str,
    pub command_path: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub tool_name: String,
    pub cwd: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn default_config_path_is_per_agent_under_home() {
        let home = Path::new("/home/someone");
        assert_eq!(
            Agent::Copilot.default_config_path(home),
            PathBuf::from("/home/someone/.config/tool-gate-hook/copilot.toml")
        );
    }
}
