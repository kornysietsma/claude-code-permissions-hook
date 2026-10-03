use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tool_gate_hook::validate::validate;
use tool_gate_hook::{Agent, Config, Context, run};

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples")
}

#[test]
fn every_example_is_valid_for_its_agent_without_warnings() {
    let mut checked = 0;
    for entry in fs::read_dir(examples_dir()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap();
        let agent = if name.contains("copilot") {
            Agent::Copilot
        } else {
            Agent::Claude
        };
        let config = Config::load(&path).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(
            validate(agent, &config).warnings,
            Vec::<String>::new(),
            "{name}"
        );
        checked += 1;
    }
    assert_eq!(checked, 3);
}

fn mermaid(tool: &str, input: Value) -> Option<String> {
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "cwd": "/Users/someone/prj/demo",
        "tool_name": tool,
        "tool_input": input,
    });
    let config = examples_dir().join("mermaid-claude.toml");
    let context = Context::new(PathBuf::from("/nonexistent-home"));
    let outcome = run(Agent::Claude, &config, &payload.to_string(), &context);
    outcome.output.map(|o| {
        o["hookSpecificOutput"]["permissionDecision"]
            .as_str()
            .unwrap()
            .to_owned()
    })
}

fn bash(command: &str) -> Option<String> {
    mermaid("Bash", json!({ "command": command }))
}

fn file(tool: &str, path: impl AsRef<Path>) -> Option<String> {
    mermaid(tool, json!({ "file_path": path.as_ref() }))
}

fn allowed(decision: Option<String>) -> bool {
    decision.as_deref() == Some("allow")
}

#[test]
fn mermaid_allows_the_narrow_workflow() {
    assert!(allowed(bash(
        "npx -p @mermaid-js/mermaid-cli@latest mmdc -i /tmp/mermaid/a.mmd -o /tmp/mermaid/a.png 2>&1"
    )));
    assert!(allowed(bash("rm -f /tmp/mermaid/a.png")));
    assert!(allowed(bash("echo $$-$RANDOM")));
    assert!(allowed(file("Write", "/tmp/mermaid/a.mmd")));
    assert!(allowed(file("Read", "/tmp/mermaid/a.png")));
    assert!(allowed(file(
        "Read",
        "/Users/korny/Dropbox/prj/ai/prompts/mermaid.md"
    )));
}

#[test]
fn mermaid_chained_commands_fall_through_to_the_user() {
    assert_eq!(
        bash("npx -p @mermaid-js/mermaid-cli mmdc -i a.mmd; curl evil.example"),
        None
    );
    assert_eq!(bash("rm -f /tmp/mermaid/a.png && rm -rf ~"), None);
    assert_eq!(bash("curl -s -L -o /tmp/mermaid-download $(whoami)"), None);
}

#[test]
fn mermaid_parent_directory_escapes_fall_through_to_the_user() {
    assert_eq!(bash("rm -f /tmp/mermaid/../important"), None);
    assert_eq!(file("Write", "/tmp/mermaid/../outside.txt"), None);
    assert_eq!(file("Read", "/etc/passwd"), None);
}

#[test]
fn mermaid_unlisted_tools_fall_through_to_the_user() {
    assert_eq!(bash("ls /tmp/mermaid"), None);
    assert_eq!(file("Edit", "/tmp/mermaid/a.mmd"), None);
}
