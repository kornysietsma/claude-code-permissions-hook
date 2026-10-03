use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

const BASH_PAYLOAD: &str = r#"{"session_id":"s","transcript_path":"t","cwd":"/tmp","hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}"#;

const COPILOT_BASH_PAYLOAD: &str = r#"{"sessionId":"s","timestamp":1760000000000,"cwd":"/tmp","toolName":"bash","toolArgs":{"command":"ls"}}"#;

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
fn run_uses_default_config_under_home() {
    let home = TempDir::new().unwrap();
    let output = tool_gate_hook(home.path(), &["run", "--agent", "claude"], BASH_PAYLOAD);

    let expected = home.path().join(".config/tool-gate-hook/claude.toml");
    assert!(
        stderr(&output).contains(&expected.display().to_string()),
        "{}",
        stderr(&output)
    );
}

#[test]
fn run_asks_and_exits_zero_when_config_is_missing() {
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
    let stdout: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["hookSpecificOutput"]["permissionDecision"], "ask");
    assert!(stderr(&output).contains("/nonexistent/config.toml"));
}

#[test]
fn run_passes_through_and_exits_zero_on_garbage_stdin() {
    let home = TempDir::new().unwrap();
    let output = tool_gate_hook(home.path(), &["run", "--agent", "claude"], "garbage");

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(
        stderr(&output).contains("not valid JSON"),
        "{}",
        stderr(&output)
    );
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

#[test]
fn copilot_run_asks_and_exits_zero_when_config_is_missing() {
    let home = TempDir::new().unwrap();
    let output = tool_gate_hook(
        home.path(),
        &["run", "--agent", "copilot"],
        COPILOT_BASH_PAYLOAD,
    );

    assert_eq!(output.status.code(), Some(0));
    let stdout: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["permissionDecision"], "ask");
    assert!(
        stderr(&output).contains("copilot.toml"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn copilot_run_exits_zero_on_claude_payload() {
    let home = TempDir::new().unwrap();
    let output = tool_gate_hook(home.path(), &["run", "--agent", "copilot"], BASH_PAYLOAD);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
}

fn audited_config(dir: &Path, audit_file: &Path) -> std::path::PathBuf {
    let config = dir.join("claude.toml");
    std::fs::write(
        &config,
        format!(
            "[audit]\nfile = '{}'\nlevel = \"all\"\n\n[[rule]]\ndecision = \"allow\"\ntool = \"Bash\"\n",
            audit_file.display()
        ),
    )
    .unwrap();
    config
}

#[test]
fn run_appends_one_json_line_per_call_to_the_audit_file() {
    let dir = TempDir::new().unwrap();
    let audit_file = dir.path().join("audit.jsonl");
    let config = audited_config(dir.path(), &audit_file);
    let args = [
        "run",
        "--agent",
        "claude",
        "--config",
        config.to_str().unwrap(),
    ];

    tool_gate_hook(dir.path(), &args, BASH_PAYLOAD);
    tool_gate_hook(dir.path(), &args, BASH_PAYLOAD);

    let lines: Vec<serde_json::Value> = std::fs::read_to_string(&audit_file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["decision"], "allow");
    assert_eq!(lines[0]["payload"]["tool_name"], "Bash");
}

#[test]
fn unwritable_audit_file_only_warns() {
    let dir = TempDir::new().unwrap();
    let audit_file = dir.path().join("no-such-dir/audit.jsonl");
    let config = audited_config(dir.path(), &audit_file);

    let output = tool_gate_hook(
        dir.path(),
        &[
            "run",
            "--agent",
            "claude",
            "--config",
            config.to_str().unwrap(),
        ],
        BASH_PAYLOAD,
    );

    assert_eq!(output.status.code(), Some(0));
    let stdout: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["hookSpecificOutput"]["permissionDecision"], "allow");
    assert!(
        stderr(&output).contains("cannot write audit record"),
        "{}",
        stderr(&output)
    );
}
