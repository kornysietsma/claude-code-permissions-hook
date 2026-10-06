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
        let config = Config::load(&path, agent).unwrap_or_else(|e| panic!("{name}: {e:#}"));
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
    assert!(allowed(bash(
        "rm -f /tmp/mermaid/a.png && npx -p @mermaid-js/mermaid-cli mmdc -i /tmp/mermaid/a.mmd"
    )));
    assert!(allowed(file("Write", "/tmp/mermaid/a.mmd")));
    assert!(allowed(file("Read", "/tmp/mermaid/a.png")));
    assert!(allowed(file(
        "Read",
        "/Users/korny/Dropbox/prj/ai/prompts/mermaid.md"
    )));
}

#[test]
fn mermaid_chained_commands_are_allowed_only_if_every_part_is() {
    assert_eq!(
        bash("npx -p @mermaid-js/mermaid-cli mmdc -i a.mmd; curl evil.example"),
        None
    );
    assert_eq!(bash("rm -f /tmp/mermaid/a.png && rm -rf ~"), None);
    assert_eq!(
        bash("curl -s -L -o /tmp/mermaid-download $(whoami)").as_deref(),
        Some("ask")
    );
    assert_eq!(bash("echo $$-$RANDOM").as_deref(), Some("ask"));
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

/// The decision object from running a shell command through the agent's example config
fn example_shell_decision(agent: Agent, command: &str) -> Option<Value> {
    let payload = match agent {
        Agent::Claude => json!({
            "hook_event_name": "PreToolUse",
            "cwd": "/Users/someone/prj/demo",
            "tool_name": "Bash",
            "tool_input": { "command": command },
        }),
        Agent::Copilot => json!({
            "cwd": "/Users/someone/prj/demo",
            "toolName": "bash",
            "toolArgs": { "command": command },
        }),
    };
    let config = examples_dir().join(format!("{}.toml", agent.name()));
    let context = Context::new(PathBuf::from("/nonexistent-home"));
    let output = run(agent, &config, &payload.to_string(), &context).output?;
    match agent {
        Agent::Claude => Some(output["hookSpecificOutput"].clone()),
        Agent::Copilot => Some(output),
    }
}

fn denied_legacy_python(agent: Agent, command: &str) -> bool {
    example_shell_decision(agent, command).is_some_and(|d| {
        d["permissionDecision"] == "deny"
            && d["permissionDecisionReason"]
                .as_str()
                .is_some_and(|r| r.contains("uv run"))
    })
}

#[test]
fn legacy_python_commands_are_denied_wherever_a_command_starts() {
    for command in [
        "python script.py",
        "python3.12 -m venv .venv",
        "/usr/bin/python3 x.py",
        ".venv/bin/python x.py",
        "pip install requests",
        "pipenv install",
        "PYTHONPATH=. python x.py",
        "cd src && python x.py",
        "ls; pip3 install x",
        "echo hi | python",
        "(python x.py)",
        "ls\npython x.py",
    ] {
        for agent in [Agent::Claude, Agent::Copilot] {
            assert!(denied_legacy_python(agent, command), "{agent:?}: {command}");
        }
    }
}

#[test]
fn uv_and_unrelated_commands_are_not_denied_as_legacy_python() {
    for command in [
        "uv run script.py",
        "uv run python -c 'print(1)'",
        "uv pip install requests",
        "uvx ruff check",
        "echo python is nice",
        "git log --grep=python",
        "cat python.txt",
        "ls pythonista",
    ] {
        for agent in [Agent::Claude, Agent::Copilot] {
            assert!(
                !denied_legacy_python(agent, command),
                "{agent:?}: {command}"
            );
        }
    }
}

#[test]
fn legacy_python_in_quoted_text_is_not_denied() {
    for command in [
        "git commit -F - <<'EOF'\nUse uv\npip install is gone\nEOF",
        "git commit -m 'drop python3 x.py from the docs'",
        "echo 'pip install x'",
    ] {
        for agent in [Agent::Claude, Agent::Copilot] {
            assert!(
                !denied_legacy_python(agent, command),
                "{agent:?}: {command}"
            );
        }
    }
}

#[test]
fn legacy_python_inside_substitutions_is_not_allowed() {
    for command in ["echo $(python -V)", "echo `python -V`"] {
        for agent in [Agent::Claude, Agent::Copilot] {
            assert!(
                example_shell_decision(agent, command).is_some(),
                "{agent:?}: {command}"
            );
        }
    }
}
