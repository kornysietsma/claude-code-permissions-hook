#![forbid(unsafe_code)]
#![warn(clippy::all)]

use crate::config::AuditLevel;
use crate::hook_io::HookInput;
use chrono::{DateTime, Utc};
use log::warn;
use nix::fcntl::{Flock, FlockArg};
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

/// The outcome of permission checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
    Passthrough,
}

#[derive(Debug, Serialize)]
struct AuditEntry {
    timestamp: DateTime<Utc>,
    session_id: String,
    tool_name: String,
    tool_input: serde_json::Value,
    decision: Decision,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    cwd: String,
}

/// Write tool use to the audit file, respecting the configured audit level.
pub fn audit_tool_use(
    audit_path: &Path,
    audit_level: AuditLevel,
    input: &HookInput,
    decision: Decision,
    reason: Option<&str>,
) {
    let should_audit = match audit_level {
        AuditLevel::Off => false,
        AuditLevel::Matched => decision != Decision::Passthrough,
        AuditLevel::All => true,
    };

    if !should_audit {
        return;
    }

    if let Err(e) = try_audit_tool_use(audit_path, input, decision, reason) {
        warn!("Failed to write audit entry: {}", e);
    }
}

fn try_audit_tool_use(
    audit_path: &Path,
    input: &HookInput,
    decision: Decision,
    reason: Option<&str>,
) -> anyhow::Result<()> {
    let entry = AuditEntry {
        timestamp: Utc::now(),
        session_id: input.session_id.clone(),
        tool_name: input.tool_name.clone(),
        tool_input: input.tool_input.clone(),
        decision,
        reason: reason.map(String::from),
        cwd: input.cwd.clone(),
    };

    let json_line = serde_json::to_string(&entry)?;

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(audit_path)?;

    let mut flock = Flock::lock(file, FlockArg::LockExclusive).map_err(|(_, e)| e)?;

    writeln!(flock, "{}", json_line)?;

    flock.unlock().map_err(|(_, e)| e)?;

    Ok(())
}
