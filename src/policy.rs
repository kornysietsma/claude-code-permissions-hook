use crate::Context;
use crate::agent::{ShellTool, ToolCall};
use crate::paths;
use crate::shell::{self, Analysis, Construct, ConstructKind, Segment};
use globset::GlobMatcher;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

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
    pub construct_rules: Vec<ConstructRule>,
    pub shell_tool: ShellTool,
    pub shell: shell::Settings,
}

/// Which kind of rule (or built-in check) produced a match
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleKind {
    Rule,
    CommandRule,
    ConstructRule,
    /// A built-in check that can't be configured away
    Floor,
}

impl RuleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleKind::Rule => "rule",
            RuleKind::CommandRule => "command_rule",
            RuleKind::ConstructRule => "construct_rule",
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
    /// Command rules only: every path-like value in the segment must be under one of these
    pub paths_under: Vec<String>,
}

/// A `[[construct_rule]]`: a decision when a shell construct is present
#[derive(Debug)]
pub struct ConstructRule {
    /// 1-based position among the config file's construct rules
    pub index: usize,
    /// Never `Allow`: constructs can only raise a decision
    pub decision: Decision,
    pub construct: ConstructKind,
    pub description: Option<String>,
    pub reason: Option<String>,
    /// `redirect_write` only: writes to files under these directories don't count
    pub outside: Vec<String>,
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

/// The first match with the most restrictive decision, among those `counts` accepts
fn most_restrictive(matches: &[Match], counts: impl Fn(&Match) -> bool) -> Option<usize> {
    let decision = matches
        .iter()
        .filter(|m| counts(m))
        .map(|m| m.decision)
        .max()?;
    matches
        .iter()
        .position(|m| counts(m) && m.decision == decision)
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
                decided_by: most_restrictive(&matches, |_| true),
                matches,
                shell: None,
            };
        }

        let analysis = match lookup(payload, self.shell_tool.command_path).and_then(Value::as_str) {
            Some(command) => shell::analyse(command, &context.home, &call.cwd, &self.shell),
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
                    .filter(|rule| rule.matches_segment(segment, &value, &call.cwd, context))
                    .map(|rule| {
                        rule.to_match(Some(i + 1), &format!(" — in \"{}\"", brief(&segment.text)))
                    })
                    .collect();
                let decision = segment_matches.iter().map(|m| m.decision).max();
                matches.extend(segment_matches);
                decision
            })
            .collect::<Vec<_>>();
        matches.extend(self.construct_rules.iter().filter_map(|rule| {
            analysis
                .constructs
                .iter()
                .find(|construct| rule.matches(construct, &call.cwd, context))
                .map(|construct| rule.to_match(construct, &analysis))
        }));

        let mut checked = analysis
            .segments
            .iter()
            .zip(&segment_decisions)
            .filter(|(segment, _)| !segment.is_neutral())
            .peekable();
        let all_allowed = checked.peek().is_some()
            && checked.all(|(_, decision)| *decision == Some(Decision::Allow));
        // Floors only stop an allow: when a rule asks or denies, it decides, and what no rule
        // allows passes through despite them
        let is_rule = |m: &Match| m.kind != RuleKind::Floor;
        let decided_by = if matches
            .iter()
            .any(|m| is_rule(m) && m.decision > Decision::Allow)
        {
            most_restrictive(&matches, is_rule)
        } else if all_allowed {
            most_restrictive(&matches, |_| true)
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
        reason.push_str(&format!(" ({})", brief(detail)));
    }
    if let Some(segment) = construct.segment.and_then(|i| analysis.segments.get(i))
        && construct.detail.as_ref() != Some(&segment.source)
    {
        reason.push_str(&format!(", in \"{}\"", brief(&segment.source)));
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

/// Longest text quoted in a reason; the audit record keeps the full text
const BRIEF_LEN: usize = 100;

/// The first line of `text`, cut to `BRIEF_LEN` characters, with `…` if anything was left out
fn brief(text: &str) -> Cow<'_, str> {
    let first_line = text.lines().next().unwrap_or_default();
    match first_line.char_indices().nth(BRIEF_LEN) {
        Some((end, _)) => Cow::Owned(format!("{} …", &first_line[..end])),
        None if first_line.len() < text.len() => Cow::Owned(format!("{first_line} …")),
        None => Cow::Borrowed(text),
    }
}

/// Where relative paths in a value resolve
struct Location<'a> {
    /// The payload cwd, which `{cwd}` in configured directories stands for
    cwd: &'a Path,
    /// Every directory a relative path might be relative to
    dirs: &'a [PathBuf],
}

impl Location<'_> {
    /// Whether `path` is under one of `allowed`, whichever directory it is relative to
    fn is_under(&self, path: &str, allowed: &[String], context: &Context) -> bool {
        let allowed: Vec<PathBuf> = allowed
            .iter()
            .map(|dir| {
                let dir = paths::expand_dir(dir, &context.home, self.cwd);
                paths::resolve(&dir, self.cwd)
            })
            .collect();
        self.dirs.iter().all(|from| {
            let target = paths::resolve(Path::new(path), from);
            allowed.iter().any(|dir| target.starts_with(dir))
        })
    }
}

impl Rule {
    fn matches_call(&self, payload: &Value, call: &ToolCall, context: &Context) -> bool {
        let location = Location {
            cwd: &call.cwd,
            dirs: std::slice::from_ref(&call.cwd),
        };
        self.tool
            .as_ref()
            .is_none_or(|tool| tool.is_match(&call.tool_name))
            && self.matches_value(payload, &location, context)
    }

    /// `value` is the segment as JSON
    fn matches_segment(
        &self,
        segment: &Segment,
        value: &Value,
        cwd: &Path,
        context: &Context,
    ) -> bool {
        let location = Location {
            cwd,
            dirs: &segment.dirs,
        };
        let paths_under = || {
            self.paths_under.is_empty()
                || segment.path_like().is_some_and(|paths| {
                    paths
                        .iter()
                        .all(|path| location.is_under(path, &self.paths_under, context))
                })
        };
        self.matches_value(value, &location, context) && paths_under()
    }

    fn matches_value(&self, value: &Value, location: &Location<'_>, context: &Context) -> bool {
        self.fields
            .iter()
            .all(|field| field.matches(value, location, context))
    }

    pub fn label(&self) -> String {
        rule_label(self.kind, self.index, self.description.as_deref())
    }

    pub fn reason(&self) -> String {
        self.reason
            .clone()
            .unwrap_or_else(|| default_reason(self.decision, &self.label()))
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

impl ConstructRule {
    fn matches(&self, construct: &Construct, cwd: &Path, context: &Context) -> bool {
        let outside = || {
            let location = Location {
                cwd,
                dirs: &construct.dirs,
            };
            construct
                .target
                .as_ref()
                .is_some_and(|target| !location.is_under(target, &self.outside, context))
        };
        construct.kind == self.construct && (self.outside.is_empty() || outside())
    }

    pub fn label(&self) -> String {
        rule_label(
            RuleKind::ConstructRule,
            self.index,
            self.description.as_deref(),
        )
    }

    /// `construct` is the first one the rule matched, which the reason names
    fn to_match(&self, construct: &Construct, analysis: &Analysis) -> Match {
        let reason = self
            .reason
            .clone()
            .unwrap_or_else(|| default_reason(self.decision, &self.label()));
        let segment = construct.segment.and_then(|i| analysis.segments.get(i));
        let context = match (&construct.target, segment) {
            (Some(target), _) => format!(" — \"{}\"", brief(target)),
            (None, Some(segment)) => format!(" — in \"{}\"", brief(&segment.source)),
            (None, None) => construct
                .detail
                .as_ref()
                .map(|detail| format!(" — {}", brief(detail)))
                .unwrap_or_default(),
        };
        Match {
            kind: RuleKind::ConstructRule,
            index: Some(self.index),
            decision: self.decision,
            description: self.description.clone(),
            reason: format!("{reason}{context}"),
            segment: None,
        }
    }
}

fn default_reason(decision: Decision, label: &str) -> String {
    format!("tool-gate-hook: {} by {label}", decision.as_str())
}

impl FieldCondition {
    fn matches(&self, payload: &Value, location: &Location<'_>, context: &Context) -> bool {
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
            (FieldMatcher::Under(dirs), Some(text)) => location.is_under(text, dirs, context),
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

    #[test]
    fn brief_keeps_the_first_line_up_to_the_limit() {
        assert_eq!(brief("cargo test"), "cargo test");
        assert_eq!(brief("cat <<EOF\nbody\nEOF\n > x"), "cat <<EOF …");
        assert_eq!(brief(""), "");
        let long = "é".repeat(BRIEF_LEN + 5);
        assert_eq!(brief(&long), format!("{} …", "é".repeat(BRIEF_LEN)));
        let exact = "x".repeat(BRIEF_LEN);
        assert_eq!(brief(&exact), exact);
    }
}
