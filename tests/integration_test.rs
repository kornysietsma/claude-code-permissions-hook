//! Integration tests for the claude-code-permissions-hook library.
//!
//! These tests use the library's public API directly to test rule matching
//! logic without spawning a subprocess.

use std::path::PathBuf;

use tool_gate_hook::{Decision, HookInput, HookResult, process_hook_input, validate_config};

/// Helper to get the path to the test config
fn config_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push("test_config.toml");
    path
}

/// Helper to get the path to the example config
fn example_config_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("example.toml");
    path
}

/// Helper to get path to a test JSON file
fn test_json_path(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests");
    path.push(name);
    path
}

/// Load a test JSON file and parse it as HookInput
fn load_test_input(filename: &str) -> HookInput {
    let json = std::fs::read_to_string(test_json_path(filename))
        .unwrap_or_else(|_| panic!("Failed to read test file: {}", filename));
    serde_json::from_str(&json)
        .unwrap_or_else(|_| panic!("Failed to parse test file: {}", filename))
}

#[test]
fn test_read_allowed() {
    let input = load_test_input("read_allowed.json");
    let result = process_hook_input(&config_path(), &input).expect("Processing should succeed");

    assert_eq!(
        result.decision,
        Decision::Allow,
        "Read within allowed path should be allowed"
    );
    assert!(
        result.reason.is_some(),
        "Allow decision should have a reason"
    );
}

#[test]
fn test_read_path_traversal_denied() {
    let input = load_test_input("read_path_traversal.json");
    let result = process_hook_input(&config_path(), &input).expect("Processing should succeed");

    assert_eq!(
        result.decision,
        Decision::Deny,
        "Path traversal should be denied"
    );
    assert!(
        result.reason.is_some(),
        "Deny decision should have a reason"
    );
}

#[test]
fn test_bash_injection_denied() {
    let input = load_test_input("bash_injection.json");
    let result = process_hook_input(&config_path(), &input).expect("Processing should succeed");

    assert_eq!(
        result.decision,
        Decision::Deny,
        "Shell injection attempt should be denied"
    );
    assert!(
        result.reason.is_some(),
        "Deny decision should have a reason"
    );
}

#[test]
fn test_bash_allowed() {
    let input = load_test_input("bash_allowed.json");
    let result = process_hook_input(&config_path(), &input).expect("Processing should succeed");

    assert_eq!(
        result.decision,
        Decision::Allow,
        "cargo test should be allowed"
    );
    assert!(
        result.reason.is_some(),
        "Allow decision should have a reason"
    );
}

#[test]
fn test_unknown_tool_passthrough() {
    let input = load_test_input("unknown_tool.json");
    let result = process_hook_input(&config_path(), &input).expect("Processing should succeed");

    assert_eq!(
        result.decision,
        Decision::Passthrough,
        "Unknown tool should passthrough"
    );
    assert!(
        result.reason.is_none(),
        "Passthrough should not have a reason"
    );
}

#[test]
fn test_validate_example_config() {
    let result = validate_config(&example_config_path());
    assert!(result.is_ok(), "Example config should be valid");

    let (deny_count, allow_count) = result.unwrap();
    assert!(deny_count > 0, "Example config should have deny rules");
    assert!(allow_count > 0, "Example config should have allow rules");
}

#[test]
fn test_validate_test_config() {
    let result = validate_config(&config_path());
    assert!(result.is_ok(), "Test config should be valid");

    let (deny_count, allow_count) = result.unwrap();
    assert!(deny_count > 0, "Test config should have deny rules");
    assert!(allow_count > 0, "Test config should have allow rules");
}

#[test]
fn test_hook_result_constructors() {
    let allow = HookResult::allow("test reason".to_string());
    assert_eq!(allow.decision, Decision::Allow);
    assert_eq!(allow.reason, Some("test reason".to_string()));

    let deny = HookResult::deny("denied".to_string());
    assert_eq!(deny.decision, Decision::Deny);
    assert_eq!(deny.reason, Some("denied".to_string()));

    let passthrough = HookResult::passthrough();
    assert_eq!(passthrough.decision, Decision::Passthrough);
    assert_eq!(passthrough.reason, None);
}
