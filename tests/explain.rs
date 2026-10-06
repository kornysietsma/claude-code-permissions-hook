//! `explain` shows the audit record a call would get, whatever the audit settings

use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tool_gate_hook::{Agent, Context, explain};

const CONFIG: &str = r#"
[audit]
file = '/nonexistent/audit.jsonl'
level = "off"
max_value_len = 5

[[command_rule]]
decision = "allow"
description = "cargo workflow"
match.text = { regex = '^cargo (build|test)\b' }

[[command_rule]]
decision = "allow"
description = "tee"
match.name = { equals = "tee" }
"#;

fn context() -> Context {
    Context::new(PathBuf::from("/nonexistent-home"))
}

fn with_config(contents: &str, test: impl FnOnce(&Path)) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, contents).unwrap();
    test(&path);
}

fn shell_input(agent: Agent, command: &str) -> String {
    agent
        .shell_payload(command, Path::new("/tmp/project"))
        .to_string()
}

#[test]
fn shell_payloads_are_built_for_each_agent() {
    assert_eq!(
        Agent::Claude.shell_payload("ls", Path::new("/p")),
        json!({
            "hook_event_name": "PreToolUse",
            "cwd": "/p",
            "tool_name": "Bash",
            "tool_input": { "command": "ls" }
        })
    );
    assert_eq!(
        Agent::Copilot.shell_payload("ls", Path::new("/p")),
        json!({
            "cwd": "/p",
            "toolName": "bash",
            "toolArgs": { "command": "ls" }
        })
    );
}

#[test]
fn a_command_is_explained_in_full_despite_the_audit_settings() {
    with_config(CONFIG, |config| {
        let command = "cargo build 2>&1 && cargo test | tee out.txt";
        let record = explain(
            Agent::Claude,
            config,
            &shell_input(Agent::Claude, command),
            &context(),
        )
        .unwrap()
        .record;

        assert_eq!(record["decision"], "allow");
        assert_eq!(record["shell"]["segments"].as_array().unwrap().len(), 3);
        assert_eq!(record["payload"]["tool_input"]["command"], command);
        assert_eq!(record["shell"]["segments"][2]["text"], "tee out.txt");
    });
}

#[test]
fn copilot_commands_are_explained() {
    with_config(CONFIG, |config| {
        let record = explain(
            Agent::Copilot,
            config,
            &shell_input(Agent::Copilot, "cargo test && rm x"),
            &context(),
        )
        .unwrap()
        .record;

        assert_eq!(record["decision"], "passthrough");
        assert_eq!(record["shell"]["segments"][1]["decision"], Value::Null);
    });
}

#[test]
fn an_audit_record_is_explained_from_its_payload() {
    with_config(CONFIG, |config| {
        let record = json!({
            "ts": "2026-10-03T17:42:01.123+10:00",
            "agent": "claude",
            "decision": "passthrough",
            "matches": [],
            "payload": Agent::Claude.shell_payload("cargo test", Path::new("/tmp")),
            "duration_us": 3
        });

        let explained = explain(Agent::Claude, config, &record.to_string(), &context())
            .unwrap()
            .record;

        assert_eq!(explained["decision"], "allow");
        assert_eq!(explained["payload"], record["payload"]);
    });
}

#[test]
fn non_shell_payloads_are_explained_too() {
    with_config(CONFIG, |config| {
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "cwd": "/tmp",
            "tool_name": "Read",
            "tool_input": { "file_path": "/tmp/x" }
        });

        let record = explain(Agent::Claude, config, &payload.to_string(), &context())
            .unwrap()
            .record;

        assert_eq!(record["decision"], "passthrough");
        assert_eq!(record.get("shell"), None);
    });
}

#[test]
fn a_broken_config_is_an_error() {
    with_config("not toml [", |config| {
        let error = explain(
            Agent::Claude,
            config,
            &shell_input(Agent::Claude, "ls"),
            &context(),
        )
        .unwrap_err();

        assert!(
            format!("{error:#}").contains(&config.display().to_string()),
            "{error:#}"
        );
    });
}

#[test]
fn input_that_is_not_a_payload_is_an_error() {
    with_config(CONFIG, |config| {
        for input in ["garbage", r#"{"payload": "garbage"}"#, r#"{"cwd": "/tmp"}"#] {
            assert!(
                explain(Agent::Claude, config, input, &context()).is_err(),
                "{input}"
            );
        }
    });
}

fn explain_command(agent: Agent, command: &str) -> tool_gate_hook::Explanation {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, CONFIG).unwrap();
    explain(agent, &path, &shell_input(agent, command), &context()).unwrap()
}

#[test]
fn the_reason_the_agent_would_see_follows_the_decision() {
    let explanation = explain_command(Agent::Claude, "cargo test && ls *.rs");
    let record = explanation.record;

    assert_eq!(
        record
            .as_object()
            .unwrap()
            .keys()
            .take(6)
            .collect::<Vec<_>>(),
        vec!["ts", "agent", "config", "decision", "reason", "decided_by"]
    );
    assert_eq!(
        record["reason"],
        "tool-gate-hook: ask — a value only the shell can work out (glob in *.rs), in \"ls *.rs\""
    );
    assert_eq!(explanation.warnings, Vec::<String>::new());
}

#[test]
fn allows_have_a_reason_and_passthroughs_do_not() {
    assert_eq!(
        explain_command(Agent::Copilot, "cargo test").record["reason"],
        "tool-gate-hook: allow by command rule #1 (cargo workflow) — in \"cargo test\""
    );
    assert_eq!(
        explain_command(Agent::Copilot, "rm x").record.get("reason"),
        None
    );
}

#[test]
fn a_truncated_audit_record_asks_with_a_warning() {
    with_config(CONFIG, |config| {
        let command = "cargo test && rm -rf /…[truncated, 900 chars]";
        let record = json!({
            "decision": "allow",
            "payload": Agent::Claude.shell_payload(command, Path::new("/tmp")),
        });

        let explanation = explain(Agent::Claude, config, &record.to_string(), &context()).unwrap();

        let record = explanation.record;
        assert_eq!(record["decision"], "ask");
        assert_eq!(record.get("decided_by"), None);
        assert!(
            record["reason"].as_str().unwrap().contains("truncated"),
            "{record}"
        );
        assert_eq!(record["shell"]["segments"][0]["decision"], "allow");
        assert_eq!(explanation.warnings.len(), 1);
    });
}
