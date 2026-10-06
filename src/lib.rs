//! tool-gate-hook: a PreToolUse hook that gates agent tool use.

pub mod agent;
pub mod auditing;
pub mod config;
mod paths;
pub mod policy;
pub mod shell;
pub mod validate;

pub use agent::Agent;
pub use auditing::AuditRecord;
pub use config::Config;

use agent::ToolCall;
use anyhow::{Context as _, Result};
use auditing::Invocation;
use chrono::{DateTime, FixedOffset, Local};
use policy::Decision;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Facts about the environment, injected so that tests control them
#[derive(Debug, Clone)]
pub struct Context {
    pub home: PathBuf,
    pub clock: fn() -> DateTime<FixedOffset>,
}

impl Context {
    pub fn new(home: PathBuf) -> Self {
        Context {
            home,
            clock: || Local::now().fixed_offset(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Outcome {
    /// JSON to print on stdout; `None` passes through to the agent's normal permission flow
    pub output: Option<Value>,
    /// Problems to report on stderr
    pub warnings: Vec<String>,
    /// Where to append which record, if the config's audit level asks for one
    pub audit: Option<(PathBuf, AuditRecord)>,
}

/// Evaluates one hook payload. Never fails: errors become warnings plus a passthrough or an `ask`
pub fn run(agent: Agent, config_path: &Path, stdin: &str, context: &Context) -> Outcome {
    let started = (context.clock)();
    let invocation = || Invocation {
        agent,
        config_path,
        started,
        finished: (context.clock)(),
    };

    let (payload, call) = match parse_payload(agent, stdin) {
        Ok(parsed) => parsed,
        Err(e) => {
            // The config is loaded quietly, only to find where to record the problem
            let audit = Config::load(config_path, agent)
                .ok()
                .and_then(|config| config.audit)
                .and_then(|audit| {
                    AuditRecord::for_error(&audit, &invocation(), stdin, format!("{e:#}"))
                        .map(|record| (audit.file, record))
                });
            return Outcome {
                output: None,
                warnings: vec![format!("tool-gate-hook: ignoring payload: {e:#}")],
                audit,
            };
        }
    };

    match Config::load(config_path, agent) {
        Ok(config) => {
            let evaluation = config.policy.evaluate(&payload, &call, context);
            let audit = config
                .audit
                .filter(|audit| audit.records(!evaluation.matches.is_empty()))
                .map(|audit| {
                    let record = AuditRecord::for_evaluation(
                        &invocation(),
                        &payload,
                        &evaluation,
                        audit.max_value_len,
                    );
                    (audit.file, record)
                });
            Outcome {
                output: evaluation
                    .decided_by()
                    .map(|decided| agent.render(decided.decision, &decided.reason)),
                warnings: vec![],
                audit,
            }
        }
        Err(e) => {
            let reason = format!(
                "tool-gate-hook config error ({}): {e:#}",
                config_path.display()
            );
            Outcome {
                output: Some(agent.render(Decision::Ask, &reason)),
                warnings: vec![reason],
                audit: None,
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Explanation {
    pub record: Value,
    /// Problems to report on stderr
    pub warnings: Vec<String>,
}

/// The full audit record `run` would give `input` (a payload, or an audit record holding one),
/// whatever the config's audit settings say, plus the reason the agent would be given.
/// Unlike `run`, problems are errors.
pub fn explain(
    agent: Agent,
    config_path: &Path,
    input: &str,
    context: &Context,
) -> Result<Explanation> {
    let started = (context.clock)();
    let input: Value = serde_json::from_str(input).context("input is not valid JSON")?;
    let (payload, truncated) = match input.get("payload") {
        Some(payload) => (payload.clone(), auditing::is_truncated(payload)),
        None => (input, false),
    };
    let call = agent.parse(&payload)?;
    let config = Config::load(config_path, agent)
        .with_context(|| format!("invalid config {}", config_path.display()))?;
    let evaluation = config.policy.evaluate(&payload, &call, context);
    let invocation = Invocation {
        agent,
        config_path,
        started,
        finished: (context.clock)(),
    };
    let record = AuditRecord::for_explanation(&invocation, &payload, &evaluation, truncated);
    let warnings = if truncated {
        vec![format!("tool-gate-hook: {}", auditing::TRUNCATED_REASON)]
    } else {
        vec![]
    };
    Ok(Explanation {
        record: serde_json::to_value(record)?,
        warnings,
    })
}

fn parse_payload(agent: Agent, stdin: &str) -> Result<(Value, ToolCall)> {
    let payload: Value = serde_json::from_str(stdin).context("stdin is not valid JSON")?;
    let call = agent.parse(&payload)?;
    Ok((payload, call))
}
