//! tool-gate-hook: a PreToolUse hook that gates agent tool use.
//!
//! This library provides the core logic for evaluating tool use permissions
//! based on configurable allow/deny rules with regex pattern matching.

pub mod agent;
pub mod auditing;
pub mod config;
pub mod hook_io;
pub mod matcher;

use anyhow::{Context, Result};
use std::path::Path;

pub use agent::Agent;
pub use auditing::Decision;
pub use config::{Config, Rule};
pub use hook_io::{HookInput, HookOutput};
pub use matcher::check_rules;

/// Result of processing a hook input against the configured rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookResult {
    pub decision: Decision,
    pub reason: Option<String>,
}

impl HookResult {
    /// Create an allow result with a reason.
    pub fn allow(reason: String) -> Self {
        Self {
            decision: Decision::Allow,
            reason: Some(reason),
        }
    }

    /// Create a deny result with a reason.
    pub fn deny(reason: String) -> Self {
        Self {
            decision: Decision::Deny,
            reason: Some(reason),
        }
    }

    /// Create a passthrough result (no matching rule).
    pub fn passthrough() -> Self {
        Self {
            decision: Decision::Passthrough,
            reason: None,
        }
    }
}

/// Process a hook input against the rules from a config file.
///
/// Returns the decision (allow/deny/passthrough) and optional reason.
/// This is the core logic that can be tested without stdin/stdout.
pub fn process_hook_input(config_path: &Path, input: &HookInput) -> Result<HookResult> {
    let config = Config::load_from_file(config_path).context("Failed to load configuration")?;
    process_hook_input_with_config(&config, input)
}

/// Process a hook input against pre-loaded config.
///
/// Useful when you want to load the config once and process multiple inputs,
/// or for testing with custom configs.
pub fn process_hook_input_with_config(config: &Config, input: &HookInput) -> Result<HookResult> {
    let (deny_rules, allow_rules) = config.compile_rules().context("Failed to compile rules")?;

    // Check deny rules first
    if let Some(reason) = check_rules(&deny_rules, input) {
        return Ok(HookResult::deny(reason));
    }

    // Check allow rules
    if let Some(reason) = check_rules(&allow_rules, input) {
        return Ok(HookResult::allow(reason));
    }

    // No match - passthrough
    Ok(HookResult::passthrough())
}

/// Validate a configuration file.
///
/// Returns Ok with (deny_rule_count, allow_rule_count) if valid.
pub fn validate_config(config_path: &Path) -> Result<(usize, usize)> {
    let config = Config::load_from_file(config_path).context("Failed to load configuration")?;
    let (deny_rules, allow_rules) = config.compile_rules().context("Failed to compile rules")?;
    Ok((deny_rules.len(), allow_rules.len()))
}

/// Load and compile a configuration file.
///
/// Returns the Config and compiled rules (deny_rules, allow_rules).
pub fn load_config(config_path: &Path) -> Result<(Config, Vec<Rule>, Vec<Rule>)> {
    let config = Config::load_from_file(config_path).context("Failed to load configuration")?;
    let (deny_rules, allow_rules) = config.compile_rules().context("Failed to compile rules")?;
    Ok((config, deny_rules, allow_rules))
}
