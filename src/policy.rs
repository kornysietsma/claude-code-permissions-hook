use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;

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

#[derive(Debug)]
pub struct Evaluation<'a> {
    /// Every matching rule, in file order
    pub matches: Vec<&'a Rule>,
}

impl Evaluation<'_> {
    /// The first matching rule with the most restrictive decision; `None` means passthrough
    pub fn decided_by(&self) -> Option<&Rule> {
        let decision = self.matches.iter().map(|rule| rule.decision).max()?;
        self.matches
            .iter()
            .copied()
            .find(|rule| rule.decision == decision)
    }
}

impl Policy {
    pub fn evaluate(&self, payload: &Value, tool_name: &str) -> Evaluation<'_> {
        Evaluation {
            matches: self
                .rules
                .iter()
                .filter(|rule| rule.matches(payload, tool_name))
                .collect(),
        }
    }
}

impl Rule {
    fn matches(&self, payload: &Value, tool_name: &str) -> bool {
        self.tool
            .as_ref()
            .is_none_or(|tool| tool.is_match(tool_name))
            && self.fields.iter().all(|field| field.matches(payload))
    }

    pub fn reason(&self) -> String {
        self.reason.clone().unwrap_or_else(|| {
            let description = self
                .description
                .as_ref()
                .map(|d| format!(" ({d})"))
                .unwrap_or_default();
            format!(
                "tool-gate-hook: {} by rule #{}{description}",
                self.decision.as_str(),
                self.index
            )
        })
    }
}

impl FieldCondition {
    fn matches(&self, payload: &Value) -> bool {
        let Some(text) = lookup(payload, &self.path).and_then(as_text) else {
            return false;
        };
        self.matchers.iter().all(|matcher| match matcher {
            FieldMatcher::Regex(regexes) => regexes.iter().any(|r| r.is_match(&text)),
            FieldMatcher::NotRegex(regexes) => !regexes.iter().any(|r| r.is_match(&text)),
        })
    }
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Ask => "ask",
            Decision::Deny => "deny",
        }
    }
}

fn lookup<'a>(payload: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(payload, |value, key| value.get(key))
}

fn as_text(value: &Value) -> Option<Cow<'_, str>> {
    match value {
        Value::String(s) => Some(Cow::Borrowed(s)),
        Value::Number(n) => Some(Cow::Owned(n.to_string())),
        Value::Bool(b) => Some(Cow::Owned(b.to_string())),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn lookup_walks_nested_objects() {
        let payload = json!({"tool_input": {"command": "ls"}, "cwd": "/tmp"});

        assert_eq!(lookup(&payload, "tool_input.command"), Some(&json!("ls")));
        assert_eq!(lookup(&payload, "cwd"), Some(&json!("/tmp")));
        assert_eq!(lookup(&payload, "tool_input.missing"), None);
        assert_eq!(lookup(&payload, "cwd.deeper"), None);
    }
}
