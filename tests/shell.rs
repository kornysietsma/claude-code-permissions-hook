//! Shell commands are checked segment by segment

mod common;

use common::run_with_config;
use pretty_assertions::assert_eq;
use serde_json::Value;
use tool_gate_hook::{Agent, Outcome};

/// A minimal shell tool payload for `agent`
fn shell_payload(agent: Agent, command: &str) -> String {
    let payload = match agent {
        Agent::Claude => serde_json::json!({
            "hook_event_name": "PreToolUse",
            "cwd": "/tmp",
            "tool_name": "Bash",
            "tool_input": { "command": command },
        }),
        Agent::Copilot => serde_json::json!({
            "cwd": "/tmp",
            "toolName": "bash",
            "toolArgs": { "command": command },
        }),
    };
    payload.to_string()
}

const CONFIG: &str = r#"
[[command_rule]]
decision = "allow"
description = "cargo workflow"
match.text = { regex = '^cargo (build|test|check)\b' }

[[command_rule]]
decision = "allow"
description = "git commit"
match.text = { regex = '^git commit\b' }

[[command_rule]]
decision = "allow"
description = "listing"
match.name = { equals = "ls" }

[[command_rule]]
decision = "deny"
description = "recursive delete"
reason = "No recursive deletes"
match.text = { regex = '^rm -rf\b' }

[[command_rule]]
decision = "deny"
description = "legacy Python"
match.name = { regex = '(^|/)(python[0-9.]*|pip[0-9]*)$' }

[[command_rule]]
decision = "ask"
description = "git push"
match.text = { regex = '^git push\b' }
"#;

fn outcome(agent: Agent, command: &str) -> Outcome {
    run_with_config(agent, CONFIG, &shell_payload(agent, command))
}

fn output_field(agent: Agent, output: &Value, field: &str) -> String {
    let decision = match agent {
        Agent::Claude => &output["hookSpecificOutput"],
        Agent::Copilot => output,
    };
    decision[field].as_str().unwrap().to_owned()
}

/// The decision for `command` with both agents, which must agree
fn decision(command: &str) -> Option<String> {
    let [claude, copilot] = [Agent::Claude, Agent::Copilot].map(|agent| {
        outcome(agent, command)
            .output
            .map(|output| output_field(agent, &output, "permissionDecision"))
    });
    assert_eq!(claude, copilot, "{command}");
    claude
}

fn reason(command: &str) -> String {
    let output = outcome(Agent::Claude, command).output.unwrap();
    output_field(Agent::Claude, &output, "permissionDecisionReason")
}

#[test]
fn a_compound_command_is_allowed_when_every_segment_is() {
    for command in [
        "cargo build 2>&1 && cargo test",
        "cargo check; cargo test --all | ls",
        "( cargo build ) && { cargo test; }",
        "time cargo test",
        "cargo test\nls -la",
    ] {
        assert_eq!(decision(command).as_deref(), Some("allow"), "{command}");
    }
}

#[test]
fn a_segment_no_rule_matches_passes_the_whole_command_through() {
    assert_eq!(decision("foo && cargo test"), None);
    assert_eq!(decision("cargo test | grep ok"), None);
}

#[test]
fn a_denied_segment_denies_the_whole_command() {
    assert_eq!(decision("cargo test; rm -rf /").as_deref(), Some("deny"));
    assert_eq!(decision("ls && python3 x.py").as_deref(), Some("deny"));
    assert_eq!(
        reason("cargo test; rm -rf /"),
        "No recursive deletes — in \"rm -rf /\""
    );
}

#[test]
fn an_asked_segment_asks() {
    assert_eq!(decision("cargo test && git push").as_deref(), Some("ask"));
    assert_eq!(
        reason("cargo test && git push"),
        "tool-gate-hook: ask by command rule #6 (git push) — in \"git push\""
    );
}

#[test]
fn quoting_is_understood() {
    assert_eq!(
        decision("git commit -F - <<'EOF'\nTidy up\n\npip install is no longer needed\nEOF")
            .as_deref(),
        Some("allow")
    );
    assert_eq!(
        decision("git commit -m 'stop using python3; use uv'").as_deref(),
        Some("allow")
    );
    // Quoted operators are just text: one `ls` segment, so no deny
    assert_eq!(decision("ls 'a && rm -rf /'").as_deref(), Some("allow"));
}

#[test]
fn syntax_that_cannot_be_checked_asks_even_when_every_segment_is_allowed() {
    for (command, why) in [
        ("if true; then cargo test; fi", "if statement"),
        ("for f in a b; do ls; done", "for loop"),
        ("ls $HOME", "variable in $HOME"),
        ("ls *.rs", "glob in *.rs"),
        ("ls ${(f)x}", "variable in ${(f)x}"),
        ("FOO=1 cargo test", "assignment FOO"),
        ("cargo test --features $(cat f)", "command substitution"),
    ] {
        assert_eq!(decision(command).as_deref(), Some("ask"), "{command}");
        let reason = reason(command);
        assert!(
            reason
                .starts_with("tool-gate-hook: ask — shell syntax that tool-gate-hook can't check"),
            "{reason}"
        );
        assert!(reason.contains(why), "{command}: {reason}");
    }
}

#[test]
fn a_deny_beats_a_floor() {
    assert_eq!(decision("rm -rf $HOME").as_deref(), Some("deny"));
}

#[test]
fn an_unparseable_command_asks() {
    assert_eq!(decision("ls 'unterminated").as_deref(), Some("ask"));
    assert!(
        reason("ls 'unterminated")
            .starts_with("tool-gate-hook: ask — the command could not be parsed")
    );
}

#[test]
fn a_missing_command_asks() {
    let payload =
        r#"{"hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash","tool_input":{}}"#;
    let output = run_with_config(Agent::Claude, CONFIG, payload)
        .output
        .unwrap();

    assert_eq!(
        output_field(Agent::Claude, &output, "permissionDecision"),
        "ask"
    );
    assert!(
        output_field(Agent::Claude, &output, "permissionDecisionReason")
            .contains("no command string")
    );
}

#[test]
fn whole_payload_rules_can_still_deny_or_ask_shell_calls() {
    let config = format!(
        "{CONFIG}\n[[rule]]\ndecision = \"ask\"\ntool = \"Bash\"\nmatch.\"agent_id\" = {{ exists = true }}"
    );
    let mut payload: Value = serde_json::from_str(&shell_payload(Agent::Claude, "ls")).unwrap();
    payload["agent_id"] = "subagent-1".into();
    let output = run_with_config(Agent::Claude, &config, &payload.to_string())
        .output
        .unwrap();

    assert_eq!(
        output_field(Agent::Claude, &output, "permissionDecision"),
        "ask"
    );
}

#[test]
fn copilot_hooks_registered_in_claude_settings_get_shell_rules_too() {
    let stdin = common::fixture("copilot_via_claude", "bash");
    let output = run_with_config(Agent::Claude, CONFIG, &stdin)
        .output
        .unwrap();

    assert_eq!(
        output_field(Agent::Claude, &output, "permissionDecision"),
        "allow"
    );
}

fn config_error(agent: Agent, config: &str) -> String {
    let outcome = run_with_config(agent, config, &shell_payload(agent, "ls"));
    let output = outcome.output.unwrap();
    assert_eq!(output_field(agent, &output, "permissionDecision"), "ask");
    output_field(agent, &output, "permissionDecisionReason")
}

#[test]
fn an_allow_rule_that_could_match_the_shell_tool_is_a_config_error() {
    for (agent, rule) in [
        (
            Agent::Claude,
            "[[rule]]\ndecision = \"allow\"\ntool = \"Bash\"",
        ),
        (
            Agent::Claude,
            "[[rule]]\ndecision = \"allow\"\ntool = \"Read|Bash\"",
        ),
        (Agent::Claude, "[[rule]]\ndecision = \"allow\""),
        (
            Agent::Copilot,
            "[[rule]]\ndecision = \"allow\"\ntool = \"bash\"",
        ),
    ] {
        let error = config_error(agent, rule);
        assert!(
            error.contains("rule #1: an allow [[rule]] can't match the shell tool"),
            "{error}"
        );
    }
}

#[test]
fn a_command_rule_needs_conditions_and_known_keys() {
    let empty = config_error(Agent::Claude, "[[command_rule]]\ndecision = \"allow\"");
    assert!(
        empty.contains("command rule #1: no match conditions"),
        "{empty}"
    );

    let typo = config_error(
        Agent::Claude,
        "[[command_rule]]\ndecision = \"allow\"\ntool = \"Bash\"\nmatch.text = { regex = 'x' }",
    );
    assert!(typo.contains("command rule #1"), "{typo}");
    assert!(typo.contains("tool"), "{typo}");
}
