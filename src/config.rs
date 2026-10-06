use crate::agent::Agent;
use crate::policy::{Decision, FieldCondition, FieldMatcher, Policy, Rule, RuleKind, rule_label};
use crate::shell;
use anyhow::{Context, Result, anyhow, bail};
use globset::GlobBuilder;
use regex::Regex;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Config {
    pub audit: Option<AuditConfig>,
    pub pattern_names: Vec<String>,
    pub policy: Policy,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditConfig {
    pub file: PathBuf,
    #[serde(default)]
    pub level: AuditLevel,
    #[serde(default = "default_max_value_len")]
    pub max_value_len: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditLevel {
    Off,
    #[default]
    Matched,
    All,
}

impl AuditLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            AuditLevel::Off => "off",
            AuditLevel::Matched => "matched",
            AuditLevel::All => "all",
        }
    }
}

fn default_max_value_len() -> usize {
    1024
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    audit: Option<AuditConfig>,
    #[serde(default)]
    patterns: BTreeMap<String, String>,
    #[serde(default)]
    shell: RawShell,
    // Kept as tables so each rule can be parsed with its index in error messages
    #[serde(default, rename = "rule")]
    rules: Vec<toml::Table>,
    #[serde(default, rename = "command_rule")]
    command_rules: Vec<toml::Table>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawShell {
    #[serde(default)]
    safe_env: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    decision: Decision,
    tool: Option<String>,
    description: Option<String>,
    reason: Option<String>,
    #[serde(default, rename = "match")]
    fields: BTreeMap<String, RawFieldMatch>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCommandRule {
    decision: Decision,
    description: Option<String>,
    reason: Option<String>,
    #[serde(default, rename = "match")]
    fields: BTreeMap<String, RawFieldMatch>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFieldMatch {
    regex: Option<OneOrMany>,
    not_regex: Option<OneOrMany>,
    equals: Option<String>,
    glob: Option<String>,
    exists: Option<bool>,
    under: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(untagged, expecting = "expected a string or a list of strings")]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(item) => vec![item],
            OneOrMany::Many(items) => items,
        }
    }
}

impl Config {
    pub fn load(path: &Path, agent: Agent) -> Result<Self> {
        let contents = fs::read_to_string(path).context("cannot read file")?;
        Self::from_toml(&contents, agent)
    }

    pub fn from_toml(contents: &str, agent: Agent) -> Result<Self> {
        let raw: RawConfig = toml::from_str(contents)?;
        let patterns = compile_patterns(&raw.patterns)?;
        let pattern_names = raw.patterns.keys().cloned().collect();
        let shell_tool = agent.shell_tool();
        let rules = raw
            .rules
            .into_iter()
            .enumerate()
            .map(|(i, table)| {
                let rule = compile_rule(i + 1, table, &patterns)?;
                let matches_shell = rule
                    .tool
                    .as_ref()
                    .is_none_or(|tool| tool.is_match(shell_tool.name));
                if rule.decision == Decision::Allow && matches_shell {
                    bail!(
                        "{}: an allow [[rule]] can't match the shell tool {}; \
                         allow shell commands with [[command_rule]]",
                        rule.label(),
                        shell_tool.name
                    );
                }
                Ok(rule)
            })
            .collect::<Result<_>>()?;
        let command_rules = raw
            .command_rules
            .into_iter()
            .enumerate()
            .map(|(i, table)| compile_command_rule(i + 1, table, &patterns))
            .collect::<Result<_>>()?;
        let safe_env = match raw.shell.safe_env {
            safe_env if safe_env.is_empty() => vec![],
            safe_env => {
                compile_regexes(OneOrMany::Many(safe_env), &patterns).context("[shell] safe_env")?
            }
        };
        Ok(Config {
            audit: raw.audit,
            pattern_names,
            policy: Policy {
                rules,
                command_rules,
                shell_tool,
                shell: shell::Settings { safe_env },
            },
        })
    }
}

fn compile_patterns(patterns: &BTreeMap<String, String>) -> Result<BTreeMap<&str, Regex>> {
    patterns
        .iter()
        .map(|(name, pattern)| {
            let regex = Regex::new(pattern).with_context(|| format!("pattern {name}"))?;
            Ok((name.as_str(), regex))
        })
        .collect()
}

fn label_for(kind: RuleKind, index: usize, table: &toml::Table) -> String {
    rule_label(
        kind,
        index,
        table.get("description").and_then(|d| d.as_str()),
    )
}

fn compile_rule(
    index: usize,
    table: toml::Table,
    patterns: &BTreeMap<&str, Regex>,
) -> Result<Rule> {
    let label = label_for(RuleKind::Rule, index, &table);
    let raw: RawRule = table.try_into().with_context(|| label.clone())?;
    let tool = raw
        .tool
        .map(|tool| Regex::new(&format!("^(?:{tool})$")))
        .transpose()
        .with_context(|| format!("{label}: tool"))?;
    Ok(Rule {
        kind: RuleKind::Rule,
        index,
        decision: raw.decision,
        tool,
        description: raw.description,
        reason: raw.reason,
        fields: compile_fields(raw.fields, patterns, &label)?,
    })
}

fn compile_command_rule(
    index: usize,
    table: toml::Table,
    patterns: &BTreeMap<&str, Regex>,
) -> Result<Rule> {
    let label = label_for(RuleKind::CommandRule, index, &table);
    let raw: RawCommandRule = table.try_into().with_context(|| label.clone())?;
    if raw.fields.is_empty() {
        bail!("{label}: no match conditions, so it would match every command");
    }
    Ok(Rule {
        kind: RuleKind::CommandRule,
        index,
        decision: raw.decision,
        tool: None,
        description: raw.description,
        reason: raw.reason,
        fields: compile_fields(raw.fields, patterns, &label)?,
    })
}

fn compile_fields(
    fields: BTreeMap<String, RawFieldMatch>,
    patterns: &BTreeMap<&str, Regex>,
    label: &str,
) -> Result<Vec<FieldCondition>> {
    fields
        .into_iter()
        .map(|(path, field)| {
            let matchers = compile_field(field, patterns)
                .with_context(|| format!("{label}: match.\"{path}\""))?;
            Ok(FieldCondition { path, matchers })
        })
        .collect()
}

fn compile_field(
    field: RawFieldMatch,
    patterns: &BTreeMap<&str, Regex>,
) -> Result<Vec<FieldMatcher>> {
    let mut matchers = Vec::new();
    if let Some(regex) = field.regex {
        matchers.push(FieldMatcher::Regex(
            compile_regexes(regex, patterns).context("regex")?,
        ));
    }
    if let Some(not_regex) = field.not_regex {
        matchers.push(FieldMatcher::NotRegex(
            compile_regexes(not_regex, patterns).context("not_regex")?,
        ));
    }
    if let Some(equals) = field.equals {
        matchers.push(FieldMatcher::Equals(equals));
    }
    if let Some(glob) = field.glob {
        let matcher = GlobBuilder::new(&glob)
            .literal_separator(true)
            .build()
            .context("glob")?
            .compile_matcher();
        matchers.push(FieldMatcher::Glob(matcher));
    }
    if let Some(under) = field.under {
        if under.is_empty() {
            bail!("under: empty list");
        }
        matchers.push(FieldMatcher::Under(under));
    }
    if let Some(exists) = field.exists {
        matchers.push(FieldMatcher::Exists(exists));
    }
    if matchers.is_empty() {
        bail!("no matchers given");
    }
    Ok(matchers)
}

fn compile_regexes(items: OneOrMany, patterns: &BTreeMap<&str, Regex>) -> Result<Vec<Regex>> {
    let items = items.into_vec();
    if items.is_empty() {
        bail!("empty list");
    }
    items
        .iter()
        .map(|item| match item.strip_prefix('@') {
            Some(name) => patterns
                .get(name)
                .cloned()
                .ok_or_else(|| anyhow!("unknown pattern @{name}")),
            None => Ok(Regex::new(item)?),
        })
        .collect()
}
