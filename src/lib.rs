//! tool-gate-hook: a PreToolUse hook that gates agent tool use.

pub mod agent;
pub mod auditing;
pub mod config;
pub mod policy;

pub use agent::Agent;
pub use config::Config;

use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Evaluates one hook payload; returns the JSON to print, or `None` to pass through
pub fn run(agent: Agent, config_path: &Path, stdin: &str) -> Result<Option<Value>> {
    let config = Config::load(config_path)?;
    let payload: Value = serde_json::from_str(stdin).context("stdin is not valid JSON")?;
    let tool_name = agent.tool_name(&payload)?;

    let evaluation = config.policy.evaluate(&payload, &tool_name);
    Ok(evaluation
        .decided_by()
        .map(|rule| agent.render(rule.decision, &rule.reason())))
}
