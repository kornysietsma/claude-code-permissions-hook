mod common;

use chrono::{DateTime, FixedOffset};
use common::{no_home, run_with_config};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tool_gate_hook::{Agent, Context, Outcome, run};

fn fixture(name: &str) -> String {
    common::fixture("claude", name)
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
    run_claude_with(&format!("level = \"{audit_level}\""), rules, stdin)
}

fn run_claude_with(audit_settings: &str, rules: &str, stdin: &str) -> Run {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("claude.toml");
    let audit_file = dir.path().join("audit.jsonl");
    fs::write(
        &config_path,
        format!(
            "[audit]\nfile = '{}'\n{audit_settings}\n{rules}",
            audit_file.display()
        ),
    )
    .unwrap();
    let context = Context {
        clock: fixed_clock,
        ..no_home()
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
[[command_rule]]
decision = "allow"
description = "any bash"
match.name = { exists = true }
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
            "decided_by": { "kind": "command_rule", "index": 1, "description": "any bash" },
            "matches": [
                { "kind": "command_rule", "index": 1, "decision": "allow", "description": "any bash", "segment": 1 }
            ],
            "shell": {
                "segments": [
                    {
                        "text": "cargo test --all", "name": "cargo", "args": ["test", "--all"],
                        "env": {}, "redirects": [], "wrappers": [], "source": "cargo test --all",
                        "matches": [
                            { "kind": "command_rule", "index": 1, "decision": "allow", "description": "any bash" }
                        ],
                        "decision": "allow"
                    }
                ],
                "constructs": []
            },
            "payload": serde_json::from_str::<Value>(&fixture("bash")).unwrap(),
            "duration_us": 0
        }))
    );
}

fn bash_with_command(command: &str) -> String {
    let mut payload: Value = serde_json::from_str(&fixture("bash")).unwrap();
    payload["tool_input"]["command"] = json!(command);
    payload.to_string()
}

#[test]
fn shell_record_shows_each_segment_and_construct() {
    let rules = r#"
[[command_rule]]
decision = "allow"
description = "cargo build"
match.text = { regex = '^cargo build\b' }

[[command_rule]]
decision = "ask"
description = "git push"
match.text = { regex = '^git push\b' }
"#;
    let stdin = bash_with_command("cargo build 2>&1 && git push && ls $HOME > out.txt");
    let record = run_claude("matched", rules, &stdin).record().unwrap();

    assert_eq!(
        record["shell"],
        json!({
            "segments": [
                {
                    "text": "cargo build", "name": "cargo", "args": ["build"], "env": {},
                    "redirects": [ { "op": ">&", "fd": 2, "target": "1" } ], "wrappers": [],
                    "source": "cargo build 2>& 1",
                    "matches": [
                        { "kind": "command_rule", "index": 1, "decision": "allow", "description": "cargo build" }
                    ],
                    "decision": "allow"
                },
                {
                    "text": "git push", "name": "git", "args": ["push"], "env": {},
                    "redirects": [], "wrappers": [], "source": "git push",
                    "matches": [
                        { "kind": "command_rule", "index": 2, "decision": "ask", "description": "git push" }
                    ],
                    "decision": "ask"
                },
                {
                    "text": "ls '$HOME'", "name": "ls", "args": ["$HOME"], "env": {},
                    "redirects": [ { "op": ">", "target": "out.txt" } ], "wrappers": [],
                    "source": "ls $HOME > out.txt",
                    "matches": [],
                    "decision": null
                }
            ],
            "constructs": [
                { "construct": "expansion", "segment": 3, "detail": "variable in $HOME", "floor": true },
                { "construct": "redirect_write", "segment": 3, "target": "out.txt", "floor": false }
            ]
        })
    );
    assert_eq!(
        record["decided_by"],
        json!({ "kind": "floor", "description": "expansion" })
    );
}

#[test]
fn constructs_without_a_segment_leave_it_out() {
    let record = run_claude("matched", ALLOW_BASH, &bash_with_command("echo 'open"))
        .record()
        .unwrap();

    assert_eq!(record["shell"]["segments"], json!([]));
    assert_eq!(
        record["shell"]["constructs"][0]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        vec!["construct", "detail", "floor"]
    );
}

#[test]
fn non_shell_records_have_no_shell_object() {
    let record = run_claude("all", ALLOW_BASH, &fixture("read"))
        .record()
        .unwrap();

    assert_eq!(record.get("shell"), None);
}

#[test]
fn strings_in_the_shell_object_are_truncated() {
    let long = "x".repeat(30);
    let stdin = bash_with_command(&format!("echo {long}"));
    let record = run_claude_with("level = \"all\"\nmax_value_len = 10", ALLOW_BASH, &stdin)
        .record()
        .unwrap();

    let cut = format!("{}…[truncated, 30 chars]", "x".repeat(10));
    let segment = &record["shell"]["segments"][0];
    assert_eq!(segment["args"], json!([cut]));
    assert_eq!(segment["name"], "echo");
    assert_eq!(segment["text"], "echo xxxxx…[truncated, 35 chars]");
}

#[test]
fn deny_wins_and_all_matches_are_recorded_rules_first() {
    let rules = r#"
[[command_rule]]
decision = "allow"
description = "any bash"
match.name = { exists = true }

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
        json!({ "kind": "rule", "index": 2, "description": "no bash" })
    );
    assert_eq!(
        record["matches"],
        json!([
            { "kind": "rule", "index": 1, "decision": "ask" },
            { "kind": "rule", "index": 2, "decision": "deny", "description": "no bash" },
            { "kind": "command_rule", "index": 1, "decision": "allow", "description": "any bash", "segment": 1 }
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
    assert_eq!(record["decided_by"], json!({ "kind": "rule", "index": 1 }));
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
    let outcome = run_with_config(Agent::Claude, ALLOW_BASH, &fixture("bash"));

    assert!(outcome.output.is_some());
    assert_eq!(outcome.audit, None);
}

fn write_with_content(content: &str) -> String {
    let mut payload: Value = serde_json::from_str(&fixture("write")).unwrap();
    payload["tool_input"]["content"] = json!(content);
    payload.to_string()
}

const ALLOW_WRITE: &str = "[[rule]]\ndecision = \"allow\"\ntool = \"Write\"";

#[test]
fn long_strings_are_truncated_but_every_key_is_kept() {
    let stdin = write_with_content(&"x".repeat(200));
    let record = run_claude_with("level = \"all\"\nmax_value_len = 100", ALLOW_WRITE, &stdin)
        .record()
        .unwrap();

    let mut expected: Value = serde_json::from_str(&stdin).unwrap();
    expected["tool_input"]["content"] =
        json!(format!("{}…[truncated, 200 chars]", "x".repeat(100)));
    assert_eq!(record["payload"], expected);
}

#[test]
fn max_value_len_zero_keeps_the_full_content() {
    let stdin = write_with_content(&"x".repeat(5000));
    let record = run_claude_with("level = \"all\"\nmax_value_len = 0", ALLOW_WRITE, &stdin)
        .record()
        .unwrap();

    assert_eq!(
        record["payload"],
        serde_json::from_str::<Value>(&stdin).unwrap()
    );
}

#[test]
fn default_max_value_len_is_1024() {
    let stdin = write_with_content(&"x".repeat(2000));
    let record = run_claude("all", ALLOW_WRITE, &stdin).record().unwrap();

    let content = record["payload"]["tool_input"]["content"].as_str().unwrap();
    assert!(content.ends_with("…[truncated, 2000 chars]"), "{content}");
}

#[test]
fn malformed_stdin_gives_an_error_record_with_the_raw_text() {
    let run = run_claude("matched", ALLOW_BASH, "garbage");

    assert_eq!(run.outcome.output, None);
    let mut record = run.record().unwrap();
    let error = record.as_object_mut().unwrap().remove("error").unwrap();
    assert!(
        error
            .as_str()
            .unwrap()
            .starts_with("stdin is not valid JSON"),
        "{error}"
    );
    assert_eq!(
        record,
        json!({
            "ts": "2026-10-03T17:42:01.123+10:00",
            "agent": "claude",
            "config": run.config(),
            "decision": "passthrough",
            "matches": [],
            "payload": "garbage",
            "duration_us": 0
        })
    );
}

#[test]
fn payload_for_another_agent_gives_an_error_record_with_the_raw_text() {
    let copilot = r#"{"sessionId":"s","cwd":"/tmp","toolName":"bash","toolArgs":{}}"#;
    let record = run_claude("matched", ALLOW_BASH, copilot).record().unwrap();

    assert_eq!(record["decision"], "passthrough");
    assert_eq!(record["payload"], copilot);
    assert!(
        record["error"].as_str().unwrap().contains("tool_name"),
        "{record}"
    );
}

#[test]
fn error_record_stdin_is_truncated() {
    let stdin = "y".repeat(100);
    let record = run_claude_with("level = \"all\"\nmax_value_len = 10", ALLOW_BASH, &stdin)
        .record()
        .unwrap();

    assert_eq!(
        record["payload"],
        format!("{}…[truncated, 100 chars]", "y".repeat(10))
    );
}

#[test]
fn error_records_are_not_written_when_audit_is_off() {
    assert_eq!(run_claude("off", ALLOW_BASH, "garbage").record(), None);
}

#[test]
fn bad_payload_with_a_broken_config_warns_without_asking_or_recording() {
    let outcome = run_with_config(Agent::Claude, "not toml [", "garbage");

    assert_eq!(outcome.output, None);
    assert_eq!(outcome.audit, None);
    assert_eq!(outcome.warnings.len(), 1);
}
