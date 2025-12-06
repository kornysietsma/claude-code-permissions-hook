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

/// Maximum length for string fields in audit entries (in characters).
const MAX_STRING_LEN: usize = 256;

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

/// Recursively truncate string fields in a JSON value that exceed `max_len` characters.
fn truncate_json_strings(value: &serde_json::Value, max_len: usize) -> serde_json::Value {
    match value {
        serde_json::Value::String(s) => {
            if s.chars().count() <= max_len {
                value.clone()
            } else {
                let truncated: String = s.chars().take(max_len).collect();
                serde_json::Value::String(format!("{}…", truncated))
            }
        }
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.iter()
                .map(|v| truncate_json_strings(v, max_len))
                .collect(),
        ),
        serde_json::Value::Object(obj) => serde_json::Value::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), truncate_json_strings(v, max_len)))
                .collect(),
        ),
        // Numbers, bools, null pass through unchanged
        _ => value.clone(),
    }
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
        tool_input: truncate_json_strings(&input.tool_input, MAX_STRING_LEN),
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

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn test_truncate_short_string_unchanged() {
        let input = json!("short string");
        let result = truncate_json_strings(&input, 100);
        assert_eq!(result, input);
    }

    #[test]
    fn test_truncate_long_string() {
        let long_string = "x".repeat(200);
        let input = json!(long_string);
        let result = truncate_json_strings(&input, 50);

        let truncated = result.as_str().unwrap();
        assert!(truncated.ends_with("…"));
        assert!(truncated.starts_with("xxxxxxxxxx"));
    }

    #[test]
    fn test_truncate_preserves_object_structure() {
        let long_content = "y".repeat(200);
        let input = json!({
            "file_path": "/short/path.rs",
            "content": long_content
        });
        let result = truncate_json_strings(&input, 50);

        // Structure preserved
        assert!(result.is_object());
        let obj = result.as_object().unwrap();

        // Short field unchanged
        assert_eq!(obj.get("file_path").unwrap(), "/short/path.rs");

        // Long field truncated
        let content = obj.get("content").unwrap().as_str().unwrap();
        assert!(content.ends_with("…"));
    }

    #[test]
    fn test_truncate_nested_structures() {
        let long_string = "z".repeat(100);
        let input = json!({
            "outer": {
                "inner": long_string.clone()
            },
            "array": ["short", long_string]
        });
        let result = truncate_json_strings(&input, 20);

        // Nested object string truncated
        let inner = result["outer"]["inner"].as_str().unwrap();
        assert!(inner.ends_with("…"));

        // Array elements handled
        assert_eq!(result["array"][0], "short");
        let arr_long = result["array"][1].as_str().unwrap();
        assert!(arr_long.ends_with("…"));
    }

    #[test]
    fn test_truncate_non_strings_unchanged() {
        let input = json!({
            "number": 42,
            "bool": true,
            "null": null
        });
        let result = truncate_json_strings(&input, 10);
        assert_eq!(result, input);
    }
}
