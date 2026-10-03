//! tool-gate-hook: a PreToolUse hook that gates agent tool use.

pub mod agent;
pub mod auditing;
pub mod config;
pub mod policy;

pub use agent::Agent;
pub use config::Config;

use anyhow::{Context, Result};
use policy::Decision;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    /// JSON to print on stdout; `None` passes through to the agent's normal permission flow
    pub output: Option<Value>,
    /// Problems to report on stderr
    pub warnings: Vec<String>,
}

/// Evaluates one hook payload. Never fails: errors become warnings plus a passthrough or an `ask`
pub fn run(agent: Agent, config_path: &Path, stdin: &str) -> Outcome {
    let (payload, tool_name) = match parse_payload(agent, stdin) {
        Ok(parsed) => parsed,
        Err(e) => {
            return Outcome::passthrough_with_warning(format!(
                "tool-gate-hook: ignoring payload: {e:#}"
            ));
        }
    };

    match Config::load(config_path) {
        Ok(config) => {
            let evaluation = config.policy.evaluate(&payload, &tool_name);
            Outcome {
                output: evaluation
                    .decided_by()
                    .map(|rule| agent.render(rule.decision, &rule.reason())),
                warnings: vec![],
            }
        }
        Err(e) => {
            let reason = format!(
                "tool-gate-hook config error ({}): {e:#}",
                config_path.display()
            );
            Outcome {
                output: Some(agent.render(Decision::Ask, &reason)),
                warnings: vec![reason],
            }
        }
    }
}

fn parse_payload(agent: Agent, stdin: &str) -> Result<(Value, String)> {
    let payload: Value = serde_json::from_str(stdin).context("stdin is not valid JSON")?;
    let tool_name = agent.tool_name(&payload)?;
    Ok((payload, tool_name))
}

impl Outcome {
    pub fn passthrough_with_warning(warning: String) -> Self {
        Outcome {
            output: None,
            warnings: vec![warning],
        }
    }
}
