use chrono::{DateTime, FixedOffset};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tool_gate_hook::{Agent, Context, Outcome, run};

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/claude/{name}.json"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn fixed_clock() -> DateTime<FixedOffset> {
    DateTime::parse_from_rfc3339("2026-10-03T17:42:01.123+10:00").unwrap()
}

struct Run {
    outcome: Outcome,
    config_path: PathBuf,
    _dir: TempDir,
}

fn run_claude(audit_level: &str, rules: &str, stdin: &str) -> Run {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("claude.toml");
    let audit_file = dir.path().join("audit.jsonl");
    fs::write(
        &config_path,
        format!(
            "[audit]\nfile = '{}'\nlevel = \"{audit_level}\"\n{rules}",
            audit_file.display()
        ),
    )
    .unwrap();
    let context = Context {
        clock: fixed_clock,
        ..Context::new(PathBuf::from("/nonexistent-home"))
    };
    let outcome = run(Agent::Claude, &config_path, stdin, &context);
    Run {
        outcome,
        config_path: config_path.canonicalize().unwrap(),
        _dir: dir,
    }
}

impl Run {
    fn audit_file(&self) -> PathBuf {
        self._dir.path().join("audit.jsonl")
    }

    fn record(&self) -> Option<Value> {
        let (file, record) = self.outcome.audit.as_ref()?;
        assert_eq!(file, &self.audit_file());
        Some(serde_json::to_value(record).unwrap())
    }

    fn config(&self) -> &Path {
        &self.config_path
    }
}

const ALLOW_BASH: &str = r#"
[[rule]]
decision = "allow"
tool = "Bash"
description = "any bash"
"#;

#[test]
fn allow_record_has_every_field() {
    let run = run_claude("matched", ALLOW_BASH, &fixture("bash"));

    assert_eq!(
        run.record(),
        Some(json!({
            "ts": "2026-10-03T17:42:01.123+10:00",
            "agent": "claude",
            "config": run.config(),
            "decision": "allow",
            "decided_by": { "index": 1, "description": "any bash" },
            "matches": [ { "index": 1, "decision": "allow", "description": "any bash" } ],
            "payload": serde_json::from_str::<Value>(&fixture("bash")).unwrap(),
            "duration_us": 0
        }))
    );
}

#[test]
fn deny_wins_and_all_matches_are_recorded_in_file_order() {
    let rules = r#"
[[rule]]
decision = "allow"
tool = "Bash"
description = "any bash"

[[rule]]
decision = "ask"
tool = "Bash"

[[rule]]
decision = "deny"
tool = "Bash"
description = "no bash"

[[rule]]
decision = "deny"
tool = "Read"
"#;
    let record = run_claude("matched", rules, &fixture("bash"))
        .record()
        .unwrap();

    assert_eq!(record["decision"], "deny");
    assert_eq!(
        record["decided_by"],
        json!({ "index": 3, "description": "no bash" })
    );
    assert_eq!(
        record["matches"],
        json!([
            { "index": 1, "decision": "allow", "description": "any bash" },
            { "index": 2, "decision": "ask" },
            { "index": 3, "decision": "deny", "description": "no bash" }
        ])
    );
}

#[test]
fn ask_decision_is_recorded() {
    let rules = "[[rule]]\ndecision = \"ask\"\ntool = \"Bash\"";
    let record = run_claude("matched", rules, &fixture("bash"))
        .record()
        .unwrap();

    assert_eq!(record["decision"], "ask");
    assert_eq!(record["decided_by"], json!({ "index": 1 }));
}

#[test]
fn passthrough_is_recorded_at_all_level_without_decided_by() {
    let record = run_claude("all", ALLOW_BASH, &fixture("read"))
        .record()
        .unwrap();

    assert_eq!(record["decision"], "passthrough");
    assert_eq!(record["matches"], json!([]));
    assert_eq!(record.get("decided_by"), None);
}

#[test]
fn passthrough_is_not_recorded_at_matched_level() {
    assert_eq!(
        run_claude("matched", ALLOW_BASH, &fixture("read")).record(),
        None
    );
}

#[test]
fn matched_level_records_decisions() {
    assert!(
        run_claude("matched", ALLOW_BASH, &fixture("bash"))
            .record()
            .is_some()
    );
}

#[test]
fn off_level_records_nothing() {
    assert_eq!(
        run_claude("off", ALLOW_BASH, &fixture("bash")).record(),
        None
    );
    assert_eq!(
        run_claude("off", ALLOW_BASH, &fixture("read")).record(),
        None
    );
}

#[test]
fn no_audit_section_records_nothing() {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("claude.toml");
    fs::write(&config_path, ALLOW_BASH).unwrap();
    let context = Context {
        clock: fixed_clock,
        ..Context::new(PathBuf::from("/nonexistent-home"))
    };

    let outcome = run(Agent::Claude, &config_path, &fixture("bash"), &context);

    assert!(outcome.output.is_some());
    assert_eq!(outcome.audit, None);
}
