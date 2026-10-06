mod common;

use common::run_with_config;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tool_gate_hook::Agent;

fn fixture(name: &str) -> String {
    common::fixture("copilot", name)
}

fn run_copilot(config: &str, stdin: &str) -> Option<Value> {
    run_with_config(Agent::Copilot, config, stdin).output
}

fn decision(output: &Option<Value>) -> Option<&str> {
    output
        .as_ref()
        .map(|o| o["permissionDecision"].as_str().unwrap())
}

#[test]
fn allow_output_is_the_flat_copilot_shape() {
    let output = run_copilot(
        r#"
[[command_rule]]
decision = "allow"
reason = "cargo is fine"
match.text = { regex = '^cargo ' }
"#,
        &fixture("bash"),
    );

    assert_eq!(
        output,
        Some(json!({
            "permissionDecision": "allow",
            "permissionDecisionReason": "cargo is fine — in \"cargo test\""
        }))
    );
}

#[test]
fn deny_and_ask_always_carry_a_reason() {
    let deny = run_copilot(
        "[[rule]]\ndecision = \"deny\"\ntool = \"view\"",
        &fixture("view"),
    );
    let ask = run_copilot(
        "[[rule]]\ndecision = \"ask\"\ntool = \"apply_patch\"",
        &fixture("apply_patch"),
    );

    assert_eq!(
        deny,
        Some(json!({
            "permissionDecision": "deny",
            "permissionDecisionReason": "tool-gate-hook: deny by rule #1"
        }))
    );
    assert_eq!(
        ask,
        Some(json!({
            "permissionDecision": "ask",
            "permissionDecisionReason": "tool-gate-hook: ask by rule #1"
        }))
    );
}

#[test]
fn every_fixture_is_parsed_and_matched_by_its_lowercase_tool_name() {
    for tool in [
        "bash",
        "view",
        "create",
        "apply_patch",
        "rg",
        "glob",
        "task",
    ] {
        let config = format!("[[rule]]\ndecision = \"ask\"\ntool = \"{tool}\"");
        assert_eq!(
            decision(&run_copilot(&config, &fixture(tool))),
            Some("ask"),
            "{tool}"
        );
    }
}

#[test]
fn tool_names_are_case_sensitive() {
    let output = run_copilot(
        "[[rule]]\ndecision = \"allow\"\ntool = \"Bash\"",
        &fixture("bash"),
    );

    assert_eq!(output, None);
}

#[test]
fn rules_can_match_tool_args_and_cwd() {
    let config = r#"
[[rule]]
decision = "deny"
tool = "view"
match."toolArgs.path" = { under = ["{cwd}/src"] }
"#;

    assert_eq!(
        decision(&run_copilot(config, &fixture("view"))),
        Some("deny")
    );
    assert_eq!(run_copilot(config, &fixture("apply_patch")), None);
}

#[test]
fn view_and_create_both_carry_their_file_in_path() {
    let config = r#"
[[rule]]
decision = "deny"
tool = "view|create"
match."toolArgs.path" = { regex = 'notes2\.txt$' }
"#;

    assert_eq!(
        decision(&run_copilot(config, &fixture("create"))),
        Some("deny")
    );
    assert_eq!(run_copilot(config, &fixture("view")), None);
}

#[test]
fn rules_can_match_a_patch_given_as_a_string() {
    let config = r#"
[[rule]]
decision = "deny"
tool = "apply_patch"
match."toolArgs" = { regex = '(?m)^\*\*\* (Add|Update|Delete) File: .*\.(env|secret)$' }
"#;
    let mut secret: Value = serde_json::from_str(&fixture("apply_patch")).unwrap();
    secret["toolArgs"] = json!("*** Begin Patch\n*** Add File: .env\n+KEY=1\n*** End Patch\n");

    assert_eq!(
        decision(&run_copilot(config, &secret.to_string())),
        Some("deny")
    );
    assert_eq!(run_copilot(config, &fixture("apply_patch")), None);
}

#[test]
fn search_tools_carry_their_directory_in_paths() {
    let config = r#"
[[rule]]
decision = "allow"
tool = "glob|rg"
match."toolArgs.paths" = { under = ["{cwd}"] }
"#;

    for tool in ["glob", "rg"] {
        assert_eq!(
            decision(&run_copilot(config, &fixture(tool))),
            Some("allow"),
            "{tool}"
        );
    }
}

#[test]
fn claude_agent_accepts_what_copilot_sends_to_a_claude_hook() {
    let config = r#"
[[command_rule]]
decision = "allow"
match.text = { regex = '^ls ' }

[[rule]]
decision = "ask"
tool = "Read"
match."tool_input.path" = { under = ["{cwd}"] }

[[rule]]
decision = "deny"
tool = "Edit"
match."tool_input" = { regex = '\*\*\* Update File: src/' }
"#;
    let run_via_claude = |name: &str| {
        let stdin = common::fixture("copilot_via_claude", name);
        let output = run_with_config(Agent::Claude, config, &stdin)
            .output
            .unwrap();
        output["hookSpecificOutput"]["permissionDecision"]
            .as_str()
            .unwrap()
            .to_owned()
    };

    assert_eq!(run_via_claude("bash"), "allow");
    assert_eq!(run_via_claude("read"), "ask");
    assert_eq!(run_via_claude("edit"), "deny");
}

#[test]
fn config_error_asks_in_copilot_format() {
    let outcome = run_with_config(
        Agent::Copilot,
        "[[rule]]\ndecision = \"maybe\"",
        &fixture("bash"),
    );

    let output = outcome.output.unwrap();
    assert_eq!(output["permissionDecision"], "ask");
    assert!(
        output["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .starts_with("tool-gate-hook config error ("),
        "{output}"
    );
    assert!(output.get("hookSpecificOutput").is_none());
}

#[test]
fn claude_payload_under_copilot_passes_through_even_with_a_broken_config() {
    let outcome = run_with_config(
        Agent::Copilot,
        "not toml [",
        &common::fixture("claude", "bash"),
    );

    assert_eq!(outcome.output, None);
    assert_eq!(outcome.warnings.len(), 1);
    assert!(
        outcome.warnings[0].contains("toolName"),
        "{:?}",
        outcome.warnings
    );
}

#[test]
fn copilot_payload_under_claude_passes_through() {
    let outcome = run_with_config(
        Agent::Claude,
        "[[rule]]\ndecision = \"deny\"",
        &fixture("bash"),
    );

    assert_eq!(outcome.output, None);
    assert_eq!(outcome.warnings.len(), 1);
    assert!(
        outcome.warnings[0].contains("tool_name"),
        "{:?}",
        outcome.warnings
    );
}
