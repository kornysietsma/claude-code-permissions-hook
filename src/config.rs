use crate::policy::{Decision, FieldCondition, FieldMatcher, Policy, Rule};
use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Config {
    pub audit: Option<AuditConfig>,
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

fn default_max_value_len() -> usize {
    1024
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    audit: Option<AuditConfig>,
    #[serde(default)]
    patterns: BTreeMap<String, String>,
    // Kept as tables so each rule can be parsed with its index in error messages
    #[serde(default, rename = "rule")]
    rules: Vec<toml::Table>,
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
struct RawFieldMatch {
    regex: Option<OneOrMany>,
    not_regex: Option<OneOrMany>,
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
    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path).context("cannot read file")?;
        Self::from_toml(&contents)
    }

    pub fn from_toml(contents: &str) -> Result<Self> {
        let raw: RawConfig = toml::from_str(contents)?;
        let patterns = compile_patterns(&raw.patterns)?;
        let rules = raw
            .rules
            .into_iter()
            .enumerate()
            .map(|(i, table)| compile_rule(i + 1, table, &patterns))
            .collect::<Result<_>>()?;
        Ok(Config {
            audit: raw.audit,
            policy: Policy { rules },
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

fn compile_rule(
    index: usize,
    table: toml::Table,
    patterns: &BTreeMap<&str, Regex>,
) -> Result<Rule> {
    let description = table
        .get("description")
        .and_then(|d| d.as_str())
        .map(|d| format!(" ({d})"))
        .unwrap_or_default();
    let context = || format!("rule #{index}{description}");

    let raw: RawRule = table.try_into().with_context(context)?;
    let tool = raw
        .tool
        .map(|tool| Regex::new(&format!("^(?:{tool})$")))
        .transpose()
        .with_context(|| format!("{}: tool", context()))?;
    let fields = raw
        .fields
        .into_iter()
        .map(|(path, field)| {
            let matchers = compile_field(field, patterns)
                .with_context(|| format!("{}: match.\"{path}\"", context()))?;
            Ok(FieldCondition { path, matchers })
        })
        .collect::<Result<_>>()?;

    Ok(Rule {
        index,
        decision: raw.decision,
        tool,
        description: raw.description,
        reason: raw.reason,
        fields,
    })
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
