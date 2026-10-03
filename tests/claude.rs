use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tool_gate_hook::{Agent, Config, Outcome, run};

fn fixture(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/claude/{name}.json"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn with_field(name: &str, path: &[&str], value: Value) -> String {
    let mut payload: Value = serde_json::from_str(&fixture(name)).unwrap();
    let (last, parents) = path.split_last().unwrap();
    let target = parents.iter().fold(&mut payload, |v, key| &mut v[*key]);
    target[*last] = value;
    payload.to_string()
}

fn run_claude_outcome(config: &str, stdin: &str) -> Outcome {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("claude.toml");
    fs::write(&config_path, config).unwrap();
    run(Agent::Claude, &config_path, stdin)
}

fn run_claude(config: &str, stdin: &str) -> Option<Value> {
    run_claude_outcome(config, stdin).output
}

fn decision(output: &Option<Value>) -> Option<&str> {
    output.as_ref().map(|o| {
        o["hookSpecificOutput"]["permissionDecision"]
            .as_str()
            .unwrap()
    })
}

fn reason(output: &Option<Value>) -> &str {
    output.as_ref().unwrap()["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap()
}

#[test]
fn allow_output_is_the_claude_hook_shape() {
    let output = run_claude(
        r#"
[[rule]]
decision = "allow"
tool = "Bash"
reason = "cargo is fine"
match."tool_input.command" = { regex = '^cargo ' }
"#,
        &fixture("bash"),
    );

    assert_eq!(
        output,
        Some(json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": "cargo is fine"
            },
            "suppressOutput": true
        }))
    );
}

#[test]
fn no_matching_rule_passes_through() {
    let output = run_claude(
        "[[rule]]\ndecision = \"deny\"\ntool = \"Write\"",
        &fixture("bash"),
    );

    assert_eq!(output, None);
}

#[test]
fn deny_beats_allow_regardless_of_file_order() {
    let config = r#"
[[rule]]
decision = "allow"
tool = "Read"

[[rule]]
decision = "deny"
tool = "Read"
match."tool_input.file_path" = { regex = '\.rs$' }
"#;
    let output = run_claude(config, &fixture("read"));

    assert_eq!(decision(&output), Some("deny"));
    assert_eq!(reason(&output), "tool-gate-hook: deny by rule #2");
}

#[test]
fn ask_beats_allow_and_reason_names_first_rule_of_winning_tier() {
    let config = r#"
[[rule]]
decision = "ask"
description = "edits need a look"
tool = "Edit"

[[rule]]
decision = "allow"
tool = "Edit|Write"

[[rule]]
decision = "ask"
tool = "Edit"
"#;
    let output = run_claude(config, &fixture("edit"));

    assert_eq!(decision(&output), Some("ask"));
    assert_eq!(
        reason(&output),
        "tool-gate-hook: ask by rule #1 (edits need a look)"
    );
}

#[test]
fn every_matching_rule_is_recorded_in_file_order() {
    let config = Config::from_toml(
        r#"
[[rule]]
decision = "deny"
tool = "Write"
[[rule]]
decision = "allow"
tool = "Bash"
[[rule]]
decision = "ask"
[[rule]]
decision = "deny"
match."tool_input.command" = { regex = 'test' }
"#,
    )
    .unwrap();
    let payload: Value = serde_json::from_str(&fixture("bash")).unwrap();
    let evaluation = config.policy.evaluate(&payload, "Bash");

    assert_eq!(
        evaluation
            .matches
            .iter()
            .map(|r| r.index)
            .collect::<Vec<_>>(),
        vec![2, 3, 4]
    );
    assert_eq!(evaluation.decided_by().map(|r| r.index), Some(4));
}

#[test]
fn not_regex_exclusion_passes_through_rather_than_denying() {
    let config = r#"
[patterns]
shell_chain = ';|&&|\|'

[[rule]]
decision = "allow"
tool = "Bash"
match."tool_input.command" = { regex = '^cargo ', not_regex = ["@shell_chain", '\$\('] }
"#;

    assert_eq!(
        decision(&run_claude(config, &fixture("bash"))),
        Some("allow")
    );
    let chained = with_field(
        "bash",
        &["tool_input", "command"],
        json!("cargo test && rm -rf /"),
    );
    assert_eq!(run_claude(config, &chained), None);
}

#[test]
fn regex_list_matches_if_any_item_matches() {
    let config = r#"
[[rule]]
decision = "deny"
match."tool_input.file_path" = { regex = ['\.env$', '\.rs$'] }
"#;

    assert_eq!(
        decision(&run_claude(config, &fixture("read"))),
        Some("deny")
    );
}

#[test]
fn missing_field_means_no_match() {
    let config = r#"
[[rule]]
decision = "deny"
match."tool_input.command" = { not_regex = 'anything' }
"#;

    assert_eq!(run_claude(config, &fixture("read")), None);
}

#[test]
fn tool_name_must_match_completely() {
    let config = "[[rule]]\ndecision = \"deny\"\ntool = \"Read\"";
    let mcp_read = with_field("read", &["tool_name"], json!("ReadMcpResource"));

    assert_eq!(run_claude(config, &mcp_read), None);
}

#[test]
fn all_match_entries_must_pass() {
    let config = r#"
[[rule]]
decision = "allow"
match."tool_input.command" = { regex = '^cargo ' }
match."permission_mode" = { regex = '^acceptEdits$' }
"#;

    assert_eq!(run_claude(config, &fixture("bash")), None);
    let accept_edits = with_field("bash", &["permission_mode"], json!("acceptEdits"));
    assert_eq!(decision(&run_claude(config, &accept_edits)), Some("allow"));
}

#[test]
fn numbers_and_booleans_match_as_json_text_but_objects_never_do() {
    let config = r#"
[[rule]]
decision = "ask"
match."tool_input.timeout" = { regex = '^120000$' }
match."tool_input.run_in_background" = { regex = '^false$' }
"#;
    assert_eq!(decision(&run_claude(config, &fixture("bash"))), Some("ask"));

    let object_config = r#"
[[rule]]
decision = "ask"
match."effort" = { regex = '.*' }
"#;
    assert_eq!(run_claude(object_config, &fixture("bash")), None);
}

#[test]
fn rules_can_target_subagents() {
    let config = r#"
[[rule]]
decision = "allow"
tool = "Agent"
description = "read-only explorer"
match."tool_input.subagent_type" = { regex = '^Explore$' }
"#;

    let output = run_claude(config, &fixture("agent"));
    assert_eq!(decision(&output), Some("allow"));
    assert_eq!(
        reason(&output),
        "tool-gate-hook: allow by rule #1 (read-only explorer)"
    );
}

#[test]
fn config_error_asks_with_the_error_as_reason() {
    let outcome = run_claude_outcome(
        r#"
[[rule]]
decision = "allow"
match."tool_input.command" = { regex = '^cargo ', not_regex = "@shell_chain" }
"#,
        &fixture("bash"),
    );

    assert_eq!(decision(&outcome.output), Some("ask"));
    let reason = reason(&outcome.output);
    assert!(
        reason.starts_with("tool-gate-hook config error ("),
        "{reason}"
    );
    assert!(reason.contains("claude.toml): "), "{reason}");
    assert!(reason.contains("unknown pattern @shell_chain"), "{reason}");
    assert_eq!(outcome.warnings, vec![reason.to_string()]);
}

#[test]
fn missing_config_file_asks() {
    let outcome = run(
        Agent::Claude,
        Path::new("/nonexistent/claude.toml"),
        &fixture("bash"),
    );

    assert_eq!(decision(&outcome.output), Some("ask"));
    assert!(reason(&outcome.output).contains("/nonexistent/claude.toml"));
}

#[test]
fn invalid_json_passes_through_with_a_warning() {
    let outcome = run_claude_outcome("", "not json {");

    assert_eq!(outcome.output, None);
    assert_eq!(outcome.warnings.len(), 1);
    assert!(
        outcome.warnings[0].contains("not valid JSON"),
        "{:?}",
        outcome.warnings
    );
}

#[test]
fn copilot_payload_passes_through_with_a_warning_even_if_config_is_broken() {
    let copilot_payload = r#"{"sessionId":"s","timestamp":1,"cwd":"/tmp","toolName":"bash","toolArgs":{"command":"ls"}}"#;
    let outcome = run_claude_outcome("not valid toml [[[", copilot_payload);

    assert_eq!(outcome.output, None);
    assert_eq!(outcome.warnings.len(), 1);
    assert!(
        outcome.warnings[0].contains("tool_name"),
        "{:?}",
        outcome.warnings
    );
}

#[test]
fn successful_decisions_have_no_warnings() {
    let outcome = run_claude_outcome("[[rule]]\ndecision = \"allow\"", &fixture("read"));

    assert_eq!(decision(&outcome.output), Some("allow"));
    assert!(outcome.warnings.is_empty());
}
