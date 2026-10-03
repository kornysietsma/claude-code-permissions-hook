use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use tool_gate_hook::{Agent, Context, Outcome, run};

fn fixture_in(agent: &str, name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/{agent}/{name}.json"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn fixture(name: &str) -> String {
    fixture_in("copilot", name)
}

fn run_outcome(agent: Agent, config: &str, stdin: &str) -> Outcome {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("config.toml");
    fs::write(&config_path, config).unwrap();
    let context = Context {
        home: PathBuf::from("/nonexistent-home"),
    };
    run(agent, &config_path, stdin, &context)
}

fn run_copilot(config: &str, stdin: &str) -> Option<Value> {
    run_outcome(Agent::Copilot, config, stdin).output
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
[[rule]]
decision = "allow"
tool = "bash"
reason = "cargo is fine"
match."toolArgs.command" = { regex = '^cargo ' }
"#,
        &fixture("bash"),
    );

    assert_eq!(
        output,
        Some(json!({
            "permissionDecision": "allow",
            "permissionDecisionReason": "cargo is fine"
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
        "[[rule]]\ndecision = \"ask\"\ntool = \"edit\"",
        &fixture("edit"),
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
    for tool in ["bash", "view", "create", "edit", "glob", "task"] {
        let config = format!("[[rule]]\ndecision = \"allow\"\ntool = \"{tool}\"");
        assert_eq!(
            decision(&run_copilot(&config, &fixture(tool))),
            Some("allow"),
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
    assert_eq!(run_copilot(config, &fixture("create")), None);
}

#[test]
fn config_error_asks_in_copilot_format() {
    let outcome = run_outcome(
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
    let outcome = run_outcome(Agent::Copilot, "not toml [", &fixture_in("claude", "bash"));

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
    let outcome = run_outcome(
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
