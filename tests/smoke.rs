use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

const BASH_PAYLOAD: &str = r#"{"session_id":"s","transcript_path":"t","cwd":"/tmp","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}"#;

fn tool_gate_hook(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tool-gate-hook"))
        .args(args)
        .env("HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn tool-gate-hook");
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(stdin.as_bytes())
        .expect("failed to write stdin");
    child
        .wait_with_output()
        .expect("failed to wait for tool-gate-hook")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn agent_flag_is_required() {
    let home = TempDir::new().unwrap();
    let output = tool_gate_hook(home.path(), &["run"], BASH_PAYLOAD);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("--agent"), "{}", stderr(&output));
}

#[test]
fn run_uses_per_agent_default_config_under_home() {
    let home = TempDir::new().unwrap();
    let output = tool_gate_hook(home.path(), &["run", "--agent", "copilot"], BASH_PAYLOAD);

    let expected = home.path().join(".config/tool-gate-hook/copilot.toml");
    assert!(
        stderr(&output).contains(&expected.display().to_string()),
        "{}",
        stderr(&output)
    );
}

#[test]
fn run_exits_zero_with_no_output_when_config_is_missing() {
    let home = TempDir::new().unwrap();
    let output = tool_gate_hook(
        home.path(),
        &[
            "run",
            "--agent",
            "claude",
            "--config",
            "/nonexistent/config.toml",
        ],
        BASH_PAYLOAD,
    );

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(stderr(&output).contains("/nonexistent/config.toml"));
}

#[test]
fn validate_fails_when_config_is_missing() {
    let home = TempDir::new().unwrap();
    let output = tool_gate_hook(home.path(), &["validate", "--agent", "claude"], "");

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("claude.toml"),
        "{}",
        stderr(&output)
    );
}
