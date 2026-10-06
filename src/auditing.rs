//! Audit records: one JSON line per hook invocation

use crate::agent::Agent;
use crate::config::{AuditConfig, AuditLevel};
use crate::policy::{Decision, Evaluation, Match, RuleKind, ShellEvaluation};
use crate::shell::Segment;
use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset, SecondsFormat};
use serde::Serialize;
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq, Serialize)]
pub struct AuditRecord {
    ts: String,
    agent: &'static str,
    config: PathBuf,
    decision: &'static str,
    /// What the agent is told; only `explain` shows it
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    decided_by: Option<DecidedBy>,
    matches: Vec<MatchedRule>,
    /// How a shell tool call's command was split up and checked
    #[serde(skip_serializing_if = "Option::is_none")]
    shell: Option<Value>,
    /// The raw stdin text instead of JSON when it couldn't be used as a payload
    payload: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    duration_us: i64,
}

#[derive(Debug, PartialEq, Serialize)]
struct DecidedBy {
    kind: RuleKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Debug, PartialEq, Serialize)]
struct MatchedRule {
    kind: RuleKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<usize>,
    decision: Decision,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    segment: Option<usize>,
}

impl From<&Match> for DecidedBy {
    fn from(m: &Match) -> Self {
        DecidedBy {
            kind: m.kind,
            index: m.index,
            description: m.description.clone(),
        }
    }
}

impl From<&Match> for MatchedRule {
    fn from(m: &Match) -> Self {
        MatchedRule {
            kind: m.kind,
            index: m.index,
            decision: m.decision,
            description: m.description.clone(),
            segment: m.segment,
        }
    }
}

#[derive(Serialize)]
struct ShellRecord<'a> {
    segments: Vec<SegmentRecord<'a>>,
    constructs: Vec<ConstructRecord<'a>>,
}

#[derive(Serialize)]
struct SegmentRecord<'a> {
    #[serde(flatten)]
    segment: &'a Segment,
    /// Command rule matches, which carry no `segment` here
    matches: Vec<MatchedRule>,
    decision: Option<Decision>,
}

#[derive(Serialize)]
struct ConstructRecord<'a> {
    construct: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    segment: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<&'a str>,
    floor: bool,
}

impl<'a> ShellRecord<'a> {
    fn new(shell: &'a ShellEvaluation, matches: &[Match]) -> Self {
        let segments = shell
            .analysis
            .segments
            .iter()
            .zip(&shell.segment_decisions)
            .enumerate()
            .map(|(i, (segment, decision))| SegmentRecord {
                segment,
                matches: matches
                    .iter()
                    .filter(|m| m.kind == RuleKind::CommandRule && m.segment == Some(i + 1))
                    .map(|m| MatchedRule {
                        segment: None,
                        ..MatchedRule::from(m)
                    })
                    .collect(),
                decision: *decision,
            })
            .collect();
        let constructs = shell
            .analysis
            .constructs
            .iter()
            .map(|construct| ConstructRecord {
                construct: construct.kind.name(),
                segment: construct.segment.map(|i| i + 1),
                detail: construct.detail.as_deref(),
                target: construct.target.as_deref(),
                floor: construct.kind.is_floor(),
            })
            .collect();
        ShellRecord {
            segments,
            constructs,
        }
    }
}

impl AuditConfig {
    /// `notable` is true when a rule matched or something went wrong
    pub fn records(&self, notable: bool) -> bool {
        match self.level {
            AuditLevel::Off => false,
            AuditLevel::Matched => notable,
            AuditLevel::All => true,
        }
    }
}

/// What the hook was asked to do, common to every kind of record
pub struct Invocation<'a> {
    pub agent: Agent,
    pub config_path: &'a Path,
    pub started: DateTime<FixedOffset>,
    pub finished: DateTime<FixedOffset>,
}

impl AuditRecord {
    /// The record for an evaluated call, with strings cut to `max_value_len` (`0`: no limit)
    pub fn for_evaluation(
        invocation: &Invocation<'_>,
        payload: &Value,
        evaluation: &Evaluation,
        max_value_len: usize,
    ) -> Self {
        let decided_by = evaluation.decided_by();
        let shell = evaluation.shell.as_ref().map(|shell| {
            let record = ShellRecord::new(shell, &evaluation.matches);
            truncate_json_strings(
                &serde_json::to_value(record).unwrap_or_default(),
                max_value_len,
            )
        });
        AuditRecord {
            decision: decided_by.map_or("passthrough", |decided| decided.decision.as_str()),
            decided_by: decided_by.map(DecidedBy::from),
            matches: evaluation.matches.iter().map(MatchedRule::from).collect(),
            shell,
            payload: truncate_json_strings(payload, max_value_len),
            ..AuditRecord::base(invocation)
        }
    }

    /// The record `explain` shows: never truncated, and with the reason the agent would be given.
    /// A `truncated` payload could have hidden anything, so it asks.
    pub fn for_explanation(
        invocation: &Invocation<'_>,
        payload: &Value,
        evaluation: &Evaluation,
        truncated: bool,
    ) -> Self {
        let record = AuditRecord::for_evaluation(invocation, payload, evaluation, 0);
        if truncated {
            AuditRecord {
                decision: Decision::Ask.as_str(),
                reason: Some(TRUNCATED_REASON.to_owned()),
                decided_by: None,
                ..record
            }
        } else {
            AuditRecord {
                reason: evaluation
                    .decided_by()
                    .map(|decided| decided.reason.clone()),
                ..record
            }
        }
    }

    /// The record for a stdin that wasn't a usable payload; written at every level but `off`
    pub fn for_error(
        config: &AuditConfig,
        invocation: &Invocation<'_>,
        stdin: &str,
        error: String,
    ) -> Option<Self> {
        config.records(true).then(|| AuditRecord {
            payload: truncate_json_strings(&Value::String(stdin.to_owned()), config.max_value_len),
            error: Some(error),
            ..AuditRecord::base(invocation)
        })
    }

    fn base(invocation: &Invocation<'_>) -> Self {
        AuditRecord {
            ts: invocation
                .started
                .to_rfc3339_opts(SecondsFormat::Millis, false),
            agent: invocation.agent.name(),
            config: invocation
                .config_path
                .canonicalize()
                .unwrap_or_else(|_| invocation.config_path.to_path_buf()),
            decision: "passthrough",
            reason: None,
            decided_by: None,
            matches: vec![],
            shell: None,
            payload: Value::Null,
            error: None,
            duration_us: (invocation.finished - invocation.started)
                .num_microseconds()
                .unwrap_or(i64::MAX),
        }
    }
}

/// Appends the record as one line, holding an exclusive lock so concurrent hooks don't interleave
pub fn append(file: &Path, record: &AuditRecord) -> Result<()> {
    let mut line = serde_json::to_string(record)?;
    line.push('\n');
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .with_context(|| format!("cannot open {}", file.display()))?;
    log.lock()
        .with_context(|| format!("cannot lock {}", file.display()))?;
    log.write_all(line.as_bytes())
        .with_context(|| format!("cannot write {}", file.display()))
}

pub const TRUNCATED_REASON: &str =
    "the audit record's payload was truncated (max_value_len), so the real call is unknown";

/// Whether any string was cut by `truncate_json_strings`
pub fn is_truncated(value: &Value) -> bool {
    match value {
        Value::String(s) => s.contains("…[truncated, ") && s.ends_with(" chars]"),
        Value::Array(items) => items.iter().any(is_truncated),
        Value::Object(fields) => fields.values().any(is_truncated),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

/// Cuts every string longer than `max_len` characters and notes the original length;
/// `0` disables truncation
pub fn truncate_json_strings(value: &Value, max_len: usize) -> Value {
    if max_len == 0 {
        return value.clone();
    }
    match value {
        Value::String(s) => {
            let len = s.chars().count();
            if len <= max_len {
                value.clone()
            } else {
                let kept: String = s.chars().take(max_len).collect();
                Value::String(format!("{kept}…[truncated, {len} chars]"))
            }
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|v| truncate_json_strings(v, max_len))
                .collect(),
        ),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), truncate_json_strings(v, max_len)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn short_strings_are_unchanged() {
        assert_eq!(truncate_json_strings(&json!("short"), 5), json!("short"));
    }

    #[test]
    fn long_strings_are_cut_and_marked_with_the_original_length() {
        assert_eq!(
            truncate_json_strings(&json!("x".repeat(200)), 50),
            json!(format!("{}…[truncated, 200 chars]", "x".repeat(50)))
        );
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        let multibyte = "é".repeat(10);

        assert_eq!(
            truncate_json_strings(&json!(multibyte), 10),
            json!(multibyte)
        );
        assert_eq!(
            truncate_json_strings(&json!(multibyte), 4),
            json!("éééé…[truncated, 10 chars]")
        );
    }

    #[test]
    fn nested_strings_are_cut_and_other_values_are_unchanged() {
        let input = json!({
            "outer": { "inner": "z".repeat(30) },
            "array": ["short", "z".repeat(30)],
            "number": 42,
            "bool": true,
            "null": null
        });

        assert_eq!(
            truncate_json_strings(&input, 20),
            json!({
                "outer": { "inner": format!("{}…[truncated, 30 chars]", "z".repeat(20)) },
                "array": ["short", format!("{}…[truncated, 30 chars]", "z".repeat(20))],
                "number": 42,
                "bool": true,
                "null": null
            })
        );
    }

    #[test]
    fn truncated_values_are_recognised() {
        let input = json!({ "a": [1, { "b": "x".repeat(30) }], "c": "short" });

        assert!(is_truncated(&truncate_json_strings(&input, 20)));
        assert!(!is_truncated(&input));
        assert!(!is_truncated(&json!("talk about …[truncated, things")));
    }

    #[test]
    fn zero_disables_truncation() {
        let input = json!({ "content": "x".repeat(5000) });

        assert_eq!(truncate_json_strings(&input, 0), input);
    }
}
