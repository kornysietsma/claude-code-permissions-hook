use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug)]
pub struct Policy {
    pub rules: Vec<Rule>,
}

#[derive(Debug)]
pub struct Rule {
    /// 1-based position in the config file
    pub index: usize,
    pub decision: Decision,
    pub tool: Option<Regex>,
    pub description: Option<String>,
    pub reason: Option<String>,
    pub fields: Vec<FieldCondition>,
}

#[derive(Debug)]
pub struct FieldCondition {
    /// Dotted path into the raw payload, e.g. `tool_input.command`
    pub path: String,
    pub matchers: Vec<FieldMatcher>,
}

#[derive(Debug)]
pub enum FieldMatcher {
    Regex(Vec<Regex>),
    NotRegex(Vec<Regex>),
}
