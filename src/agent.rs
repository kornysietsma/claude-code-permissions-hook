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

    pub fn default_config_path(self, home: &Path) -> PathBuf {
        home.join(".config")
            .join("tool-gate-hook")
            .join(format!("{}.toml", self.name()))
    }

    /// Extracts the tool name, or explains why the payload isn't from this agent
    pub fn tool_name(self, payload: &Value) -> Result<String> {
        match self {
            Agent::Claude => {
                if let Some(event) = payload.get("hook_event_name")
                    && event != "PreToolUse"
                {
                    bail!("not a PreToolUse payload (hook_event_name = {event})");
                }
                payload
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| anyhow!("no string tool_name; not a Claude Code payload"))
            }
            Agent::Copilot => bail!("Copilot support arrives in plan step 4.1"),
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
