use crate::Context;
use crate::agent::ToolCall;
use crate::paths;
use globset::GlobMatcher;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::path::Path;

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

/// Which kind of rule (or built-in check) produced a match
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    Rule,
}

impl RuleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleKind::Rule => "rule",
        }
    }
}

#[derive(Debug)]
pub struct Rule {
    pub kind: RuleKind,
    /// 1-based position among the config file's rules of this kind
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
    Equals(String),
    Glob(GlobMatcher),
    /// Unexpanded directories; `~` and `{cwd}` depend on the call being evaluated
    Under(Vec<String>),
    /// The only matcher that can pass when the field is missing
    Exists(bool),
}

/// One rule (or built-in check) that matched a call
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub kind: RuleKind,
    pub index: usize,
    pub decision: Decision,
    pub description: Option<String>,
    /// What the agent is told if this match decides the call
    pub reason: String,
}

#[derive(Debug)]
pub struct Evaluation {
    /// Every match, in evaluation order
    pub matches: Vec<Match>,
    /// Index into `matches` of the deciding match; `None` means passthrough
    decided_by: Option<usize>,
}

impl Evaluation {
    pub fn decided_by(&self) -> Option<&Match> {
        self.decided_by.map(|i| &self.matches[i])
    }
}

/// The first match with the most restrictive decision
fn most_restrictive(matches: &[Match]) -> Option<usize> {
    let decision = matches.iter().map(|m| m.decision).max()?;
    matches.iter().position(|m| m.decision == decision)
}

impl Policy {
    pub fn evaluate(&self, payload: &Value, call: &ToolCall, context: &Context) -> Evaluation {
        let matches: Vec<Match> = self
            .rules
            .iter()
            .filter(|rule| rule.matches_call(payload, call, context))
            .map(Rule::to_match)
            .collect();
        Evaluation {
            decided_by: most_restrictive(&matches),
            matches,
        }
    }
}

impl Rule {
    fn matches_call(&self, payload: &Value, call: &ToolCall, context: &Context) -> bool {
        self.tool
            .as_ref()
            .is_none_or(|tool| tool.is_match(&call.tool_name))
            && self.matches_value(payload, &call.cwd, context)
    }

    fn matches_value(&self, value: &Value, cwd: &Path, context: &Context) -> bool {
        self.fields
            .iter()
            .all(|field| field.matches(value, cwd, context))
    }

    pub fn label(&self) -> String {
        rule_label(self.kind, self.index, self.description.as_deref())
    }

    pub fn reason(&self) -> String {
        self.reason.clone().unwrap_or_else(|| {
            format!(
                "tool-gate-hook: {} by {}",
                self.decision.as_str(),
                self.label()
            )
        })
    }

    fn to_match(&self) -> Match {
        Match {
            kind: self.kind,
            index: self.index,
            decision: self.decision,
            description: self.description.clone(),
            reason: self.reason(),
        }
    }
}

impl FieldCondition {
    fn matches(&self, payload: &Value, cwd: &Path, context: &Context) -> bool {
        let value = lookup(payload, &self.path);
        let text = value.and_then(as_text);
        self.matchers.iter().all(|matcher| match (matcher, &text) {
            (FieldMatcher::Exists(expected), _) => value.is_some() == *expected,
            (_, None) => false,
            (FieldMatcher::Regex(regexes), Some(text)) => regexes.iter().any(|r| r.is_match(text)),
            (FieldMatcher::NotRegex(regexes), Some(text)) => {
                !regexes.iter().any(|r| r.is_match(text))
            }
            (FieldMatcher::Equals(expected), Some(text)) => text == expected,
            (FieldMatcher::Glob(glob), Some(text)) => glob.is_match(text.as_ref()),
            (FieldMatcher::Under(dirs), Some(text)) => {
                let target = paths::resolve(Path::new(text.as_ref()), cwd);
                dirs.iter().any(|dir| {
                    let dir = paths::expand_dir(dir, &context.home, cwd);
                    target.starts_with(paths::resolve(&dir, cwd))
                })
            }
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

/// How messages refer to a rule, e.g. `rule #2 (rm test files)`
pub(crate) fn rule_label(kind: RuleKind, index: usize, description: Option<&str>) -> String {
    let kind = kind.as_str().replace('_', " ");
    match description {
        Some(description) => format!("{kind} #{index} ({description})"),
        None => format!("{kind} #{index}"),
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
