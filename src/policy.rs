use crate::Context;
use crate::agent::{ShellTool, ToolCall};
use crate::paths;
use crate::shell::{self, Analysis, Construct};
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
    pub command_rules: Vec<Rule>,
    pub shell_tool: ShellTool,
    pub shell: shell::Settings,
}

/// Which kind of rule (or built-in check) produced a match
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    Rule,
    CommandRule,
    /// A built-in check that can't be configured away
    Floor,
}

impl RuleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleKind::Rule => "rule",
            RuleKind::CommandRule => "command_rule",
            RuleKind::Floor => "floor",
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
    /// The rule's position among its kind; `None` for floors
    pub index: Option<usize>,
    pub decision: Decision,
    /// The rule's description, or the floor's construct name
    pub description: Option<String>,
    /// What the agent is told if this match decides the call
    pub reason: String,
    /// 1-based index of the shell segment it applies to
    pub segment: Option<usize>,
}

#[derive(Debug)]
pub struct Evaluation {
    /// Every match, in evaluation order
    pub matches: Vec<Match>,
    /// Index into `matches` of the deciding match; `None` means passthrough
    decided_by: Option<usize>,
    /// Present for shell tool calls
    pub shell: Option<ShellEvaluation>,
}

#[derive(Debug)]
pub struct ShellEvaluation {
    pub analysis: Analysis,
    /// Per segment: the most restrictive matching command rule's decision, if any; always
    /// `None` for neutral segments
    pub segment_decisions: Vec<Option<Decision>>,
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
        let mut matches: Vec<Match> = self
            .rules
            .iter()
            .filter(|rule| rule.matches_call(payload, call, context))
            .map(|rule| rule.to_match(None, ""))
            .collect();
        if call.tool_name != self.shell_tool.name {
            return Evaluation {
                decided_by: most_restrictive(&matches),
                matches,
                shell: None,
            };
        }

        let analysis = match lookup(payload, self.shell_tool.command_path).and_then(Value::as_str) {
            Some(command) => shell::analyse(command, &context.home, &self.shell),
            None => Analysis::unsupported("no command string in the payload"),
        };
        matches.extend(
            analysis
                .constructs
                .iter()
                .filter(|construct| construct.kind.is_floor())
                .map(|construct| floor_match(construct, &analysis)),
        );
        let segment_decisions = analysis
            .segments
            .iter()
            .enumerate()
            .map(|(i, segment)| {
                if segment.is_neutral() {
                    return None;
                }
                let value = serde_json::to_value(segment).unwrap_or_default();
                let segment_matches: Vec<Match> = self
                    .command_rules
                    .iter()
                    .filter(|rule| rule.matches_value(&value, &call.cwd, context))
                    .map(|rule| rule.to_match(Some(i + 1), &format!(" — in \"{}\"", segment.text)))
                    .collect();
                let decision = segment_matches.iter().map(|m| m.decision).max();
                matches.extend(segment_matches);
                decision
            })
            .collect::<Vec<_>>();

        let mut checked = analysis
            .segments
            .iter()
            .zip(&segment_decisions)
            .filter(|(segment, _)| !segment.is_neutral())
            .peekable();
        let all_allowed = checked.peek().is_some()
            && checked.all(|(_, decision)| *decision == Some(Decision::Allow));
        let decided_by = if matches.iter().any(|m| m.decision > Decision::Allow) {
            most_restrictive(&matches)
        } else if all_allowed {
            matches.iter().position(|m| m.decision == Decision::Allow)
        } else {
            None
        };
        Evaluation {
            matches,
            decided_by,
            shell: Some(ShellEvaluation {
                analysis,
                segment_decisions,
            }),
        }
    }
}

fn floor_match(construct: &Construct, analysis: &Analysis) -> Match {
    let mut reason = format!("tool-gate-hook: ask — {}", construct.kind.describe());
    if let Some(detail) = &construct.detail {
        reason.push_str(&format!(" ({detail})"));
    }
    if let Some(segment) = construct.segment.and_then(|i| analysis.segments.get(i)) {
        reason.push_str(&format!(", in \"{}\"", segment.source));
    }
    Match {
        kind: RuleKind::Floor,
        index: None,
        decision: Decision::Ask,
        description: Some(construct.kind.name().to_owned()),
        reason,
        segment: construct.segment.map(|i| i + 1),
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

    /// `context` is appended to the reason, to say which shell segment matched
    fn to_match(&self, segment: Option<usize>, context: &str) -> Match {
        Match {
            kind: self.kind,
            index: Some(self.index),
            decision: self.decision,
            description: self.description.clone(),
            reason: format!("{}{context}", self.reason()),
            segment,
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
