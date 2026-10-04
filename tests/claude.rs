use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use tool_gate_hook::agent::ToolCall;
use tool_gate_hook::{Agent, Config, Context, Outcome, run};

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
    run(Agent::Claude, &config_path, stdin, &no_home())
}

fn no_home() -> Context {
    Context::new(PathBuf::from("/nonexistent-home"))
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
    let call = ToolCall {
        tool_name: "Bash".into(),
        cwd: PathBuf::from("/"),
    };
    let evaluation = config.policy.evaluate(&payload, &call, &no_home());

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
fn rules_can_target_the_subagent_handback_event() {
    let config = r#"
[[rule]]
decision = "allow"
tool = "SubagentHandback"
description = "subagent results"
match."agent_type" = { equals = "general-purpose" }
"#;

    let output = run_claude(config, &fixture("subagent_handback"));
    assert_eq!(decision(&output), Some("allow"));
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
        &no_home(),
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

#[test]
fn equals_matches_the_whole_value_only() {
    let config = r#"
[[rule]]
decision = "allow"
match."tool_input.subagent_type" = { equals = "Explore" }
"#;

    assert_eq!(
        decision(&run_claude(config, &fixture("agent"))),
        Some("allow")
    );
    let other = with_field("agent", &["tool_input", "subagent_type"], json!("Explorer"));
    assert_eq!(run_claude(config, &other), None);
}

#[test]
fn exists_checks_presence_including_absence() {
    let present = r#"
[[rule]]
decision = "ask"
match."effort" = { exists = true }
"#;
    let absent = r#"
[[rule]]
decision = "ask"
tool = "Bash"
match."agent_id" = { exists = false }
"#;

    assert_eq!(
        decision(&run_claude(present, &fixture("bash"))),
        Some("ask")
    );
    assert_eq!(decision(&run_claude(absent, &fixture("bash"))), Some("ask"));
    let in_subagent = with_field("bash", &["agent_id"], json!("subagent-001"));
    assert_eq!(run_claude(absent, &in_subagent), None);
}

#[test]
fn glob_matches_paths_and_star_does_not_cross_directories() {
    let config = r#"
[[rule]]
decision = "allow"
match."tool_input.file_path" = { glob = "/Users/someone/prj/*/src/*.rs" }
"#;

    assert_eq!(
        decision(&run_claude(config, &fixture("read"))),
        Some("allow")
    );
    let nested = with_field(
        "read",
        &["tool_input", "file_path"],
        json!("/Users/someone/prj/demo/src/deep/main.rs"),
    );
    assert_eq!(run_claude(config, &nested), None);

    let double_star = r#"
[[rule]]
decision = "allow"
match."tool_input.file_path" = { glob = "**/*.rs" }
"#;
    assert_eq!(decision(&run_claude(double_star, &nested)), Some("allow"));
}

#[test]
fn several_matchers_on_one_field_must_all_pass() {
    let config = r#"
[[rule]]
decision = "allow"
match."tool_input.file_path" = { glob = "**/*.rs", not_regex = '/deep/' }
"#;

    assert_eq!(
        decision(&run_claude(config, &fixture("read"))),
        Some("allow")
    );
    let nested = with_field(
        "read",
        &["tool_input", "file_path"],
        json!("/Users/someone/prj/demo/src/deep/main.rs"),
    );
    assert_eq!(run_claude(config, &nested), None);
}

#[test]
fn invalid_glob_is_a_config_error() {
    let config = r#"
[[rule]]
decision = "allow"
match."tool_input.file_path" = { glob = "src/[unclosed" }
"#;
    let output = run_claude(config, &fixture("read"));

    assert_eq!(decision(&output), Some("ask"));
    assert!(reason(&output).contains("glob"), "{}", reason(&output));
}

struct PathFixture {
    _root: TempDir,
    home: PathBuf,
    project: PathBuf,
    outside: PathBuf,
}

/// root/home/project/{src/main.rs, escape -> root/outside}, root/outside/secret.txt
fn path_fixture() -> PathFixture {
    let root = TempDir::new().unwrap();
    let home = root.path().join("home");
    let project = home.join("project");
    let outside = root.path().join("outside");
    fs::create_dir_all(project.join("src")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(project.join("src/main.rs"), "").unwrap();
    fs::write(outside.join("secret.txt"), "").unwrap();
    std::os::unix::fs::symlink(&outside, project.join("escape")).unwrap();
    PathFixture {
        _root: root,
        home,
        project,
        outside,
    }
}

fn read_in(paths: &PathFixture, file_path: &str) -> String {
    let payload = with_field("read", &["tool_input", "file_path"], json!(file_path));
    let mut payload: Value = serde_json::from_str(&payload).unwrap();
    payload["cwd"] = json!(paths.project);
    payload.to_string()
}

fn run_under(paths: &PathFixture, under: &str, file_path: &str) -> Option<String> {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join("claude.toml");
    fs::write(
        &config_path,
        format!("[[rule]]\ndecision = \"allow\"\nmatch.\"tool_input.file_path\" = {{ under = [{under}] }}"),
    )
    .unwrap();
    let context = Context::new(paths.home.clone());
    let outcome = run(
        Agent::Claude,
        &config_path,
        &read_in(paths, file_path),
        &context,
    );
    decision(&outcome.output).map(str::to_owned)
}

fn allowed(paths: &PathFixture, under: &str, file_path: impl AsRef<Path>) -> bool {
    run_under(paths, under, file_path.as_ref().to_str().unwrap()).as_deref() == Some("allow")
}

#[test]
fn under_allows_existing_and_not_yet_existing_files_inside_the_directory() {
    let paths = path_fixture();
    let project = format!("{:?}", paths.project);

    assert!(allowed(&paths, &project, paths.project.join("src/main.rs")));
    assert!(allowed(
        &paths,
        &project,
        paths.project.join("src/new/file.rs")
    ));
    assert!(!allowed(&paths, &project, paths.outside.join("secret.txt")));
}

#[test]
fn under_is_not_fooled_by_parent_directory_segments() {
    let paths = path_fixture();
    let project = format!("{:?}", paths.project);

    assert!(!allowed(
        &paths,
        &project,
        paths.project.join("src/../../../outside/secret.txt")
    ));
    assert!(!allowed(
        &paths,
        &project,
        paths
            .project
            .join("src/missing/../../../outside/secret.txt")
    ));
    assert!(allowed(
        &paths,
        &project,
        paths.project.join("src/../src/main.rs")
    ));
}

#[test]
fn under_follows_symlinks_out_of_the_directory() {
    let paths = path_fixture();
    let project = format!("{:?}", paths.project);

    assert!(!allowed(
        &paths,
        &project,
        paths.project.join("escape/secret.txt")
    ));
    assert!(!allowed(
        &paths,
        &project,
        paths.project.join("escape/new.txt")
    ));
    // Textual cleanup would give project/secret.txt; the OS resolves escape first, giving root/secret.txt
    assert!(!allowed(
        &paths,
        &project,
        paths.project.join("escape/../secret.txt")
    ));
}

#[test]
fn under_compares_whole_path_components() {
    let paths = path_fixture();
    let sibling = paths.home.join("project2/file.rs");

    assert!(!allowed(&paths, &format!("{:?}", paths.project), sibling));
}

#[test]
fn under_expands_cwd_and_home_and_resolves_relative_values_against_cwd() {
    let paths = path_fixture();

    assert!(allowed(
        &paths,
        r#""{cwd}""#,
        paths.project.join("src/main.rs")
    ));
    assert!(allowed(
        &paths,
        r#""~/project/src""#,
        paths.project.join("src/main.rs")
    ));
    assert!(allowed(&paths, r#""{cwd}/src""#, "src/main.rs"));
    assert!(!allowed(&paths, r#""{cwd}/src""#, "../outside.txt"));
}
