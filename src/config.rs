use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub audit: AuditConfig,
    #[serde(default)]
    pub allow: Vec<RuleConfig>,
    #[serde(default)]
    pub deny: Vec<RuleConfig>,
}

/// Controls what gets written to the audit log file.
#[derive(Debug, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuditLevel {
    /// No audit logging
    Off,
    /// Log only tool use that matches a rule (default)
    #[default]
    Matched,
    /// Log all tool use including passthrough
    All,
}

#[derive(Debug, Deserialize)]
pub struct AuditConfig {
    pub audit_file: PathBuf,
    #[serde(default)]
    pub audit_level: AuditLevel,
}

#[derive(Debug, Deserialize)]
pub struct RuleConfig {
    pub tool: String,
    pub file_path_regex: Option<String>,
    pub file_path_exclude_regex: Option<String>,
    pub command_regex: Option<String>,
    pub command_exclude_regex: Option<String>,
    pub subagent_type: Option<String>,
    pub subagent_type_exclude_regex: Option<String>,
    pub prompt_regex: Option<String>,
    pub prompt_exclude_regex: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Rule {
    pub tool: String,
    pub file_path_regex: Option<Regex>,
    pub file_path_exclude_regex: Option<Regex>,
    pub command_regex: Option<Regex>,
    pub command_exclude_regex: Option<Regex>,
    pub subagent_type: Option<String>,
    pub subagent_type_exclude_regex: Option<Regex>,
    pub prompt_regex: Option<Regex>,
    pub prompt_exclude_regex: Option<Regex>,
}

impl Config {
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse TOML config: {}", path.display()))?;

        Ok(config)
    }

    pub fn compile_rules(&self) -> Result<(Vec<Rule>, Vec<Rule>)> {
        let deny_rules = self
            .deny
            .iter()
            .map(compile_rule)
            .collect::<Result<Vec<_>>>()
            .context("Failed to compile deny rules")?;

        let allow_rules = self
            .allow
            .iter()
            .map(compile_rule)
            .collect::<Result<Vec<_>>>()
            .context("Failed to compile allow rules")?;

        Ok((deny_rules, allow_rules))
    }
}

fn compile_rule(rule_config: &RuleConfig) -> Result<Rule> {
    let file_path_regex = rule_config
        .file_path_regex
        .as_ref()
        .map(|s| Regex::new(s))
        .transpose()
        .context("Invalid file_path_regex")?;

    let file_path_exclude_regex = rule_config
        .file_path_exclude_regex
        .as_ref()
        .map(|s| Regex::new(s))
        .transpose()
        .context("Invalid file_path_exclude_regex")?;

    let command_regex = rule_config
        .command_regex
        .as_ref()
        .map(|s| Regex::new(s))
        .transpose()
        .context("Invalid command_regex")?;

    let command_exclude_regex = rule_config
        .command_exclude_regex
        .as_ref()
        .map(|s| Regex::new(s))
        .transpose()
        .context("Invalid command_exclude_regex")?;

    let subagent_type_exclude_regex = rule_config
        .subagent_type_exclude_regex
        .as_ref()
        .map(|s| Regex::new(s))
        .transpose()
        .context("Invalid subagent_type_exclude_regex")?;

    let prompt_regex = rule_config
        .prompt_regex
        .as_ref()
        .map(|s| Regex::new(s))
        .transpose()
        .context("Invalid prompt_regex")?;

    let prompt_exclude_regex = rule_config
        .prompt_exclude_regex
        .as_ref()
        .map(|s| Regex::new(s))
        .transpose()
        .context("Invalid prompt_exclude_regex")?;

    Ok(Rule {
        tool: rule_config.tool.clone(),
        file_path_regex,
        file_path_exclude_regex,
        command_regex,
        command_exclude_regex,
        subagent_type: rule_config.subagent_type.clone(),
        subagent_type_exclude_regex,
        prompt_regex,
        prompt_exclude_regex,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_compile_rule() -> Result<()> {
        let rule_config = RuleConfig {
            tool: "Read".to_string(),
            file_path_regex: Some(r"^/home/.*".to_string()),
            file_path_exclude_regex: Some(r"\.\.".to_string()),
            command_regex: None,
            command_exclude_regex: None,
            subagent_type: None,
            subagent_type_exclude_regex: None,
            prompt_regex: None,
            prompt_exclude_regex: None,
        };

        let rule = compile_rule(&rule_config)?;
        assert_eq!(rule.tool, "Read");
        assert!(rule.file_path_regex.is_some());
        assert!(rule.file_path_exclude_regex.is_some());

        Ok(())
    }
}
