use pretty_assertions::assert_eq;
use std::path::PathBuf;
use tool_gate_hook::config::{AuditConfig, AuditLevel, Config};
use tool_gate_hook::policy::{Decision, FieldMatcher};

const VALID: &str = r#"
[audit]
file = "/tmp/audit.jsonl"
level = "all"

[patterns]
shell_chain = ';|&&|\|'

[[rule]]
decision = "allow"
tool = "Bash"
description = "safe cargo"
match."tool_input.command" = { regex = '^cargo (build|test)\b', not_regex = ["@shell_chain", '\$\('] }

[[rule]]
decision = "deny"
reason = "no secrets"
match."tool_input.file_path" = { regex = ['\.env$', '\.pem$'] }

[[rule]]
decision = "ask"
tool = "Agent"
"#;

fn error_text(toml: &str) -> String {
    format!(
        "{:#}",
        Config::from_toml(toml).expect_err("config should be invalid")
    )
}

#[test]
fn valid_config_compiles_rules_in_file_order() {
    let config = Config::from_toml(VALID).unwrap();

    assert_eq!(
        config.audit,
        Some(AuditConfig {
            file: PathBuf::from("/tmp/audit.jsonl"),
            level: AuditLevel::All,
            max_value_len: 1024,
        })
    );

    let rules = &config.policy.rules;
    assert_eq!(rules.len(), 3);
    assert_eq!(
        rules
            .iter()
            .map(|r| (r.index, r.decision))
            .collect::<Vec<_>>(),
        vec![
            (1, Decision::Allow),
            (2, Decision::Deny),
            (3, Decision::Ask)
        ]
    );
    assert_eq!(rules[0].description.as_deref(), Some("safe cargo"));
    assert_eq!(rules[1].reason.as_deref(), Some("no secrets"));
    assert!(rules[1].tool.is_none());
    assert!(rules[2].fields.is_empty());

    let command = &rules[0].fields[0];
    assert_eq!(command.path, "tool_input.command");
    let FieldMatcher::NotRegex(excludes) = &command.matchers[1] else {
        panic!("expected not_regex, got {:?}", command.matchers[1]);
    };
    assert_eq!(
        excludes.iter().map(|r| r.as_str()).collect::<Vec<_>>(),
        vec![";|&&|\\|", "\\$\\("]
    );
}

#[test]
fn tool_regex_is_anchored() {
    let config = Config::from_toml(VALID).unwrap();
    let tool = config.policy.rules[0].tool.as_ref().unwrap();

    assert!(tool.is_match("Bash"));
    assert!(!tool.is_match("BashOutput"));
}

#[test]
fn audit_section_is_optional_and_defaults_apply() {
    assert_eq!(Config::from_toml("").unwrap().audit, None);

    let config = Config::from_toml("[audit]\nfile = \"/tmp/a.jsonl\"").unwrap();
    assert_eq!(
        config.audit,
        Some(AuditConfig {
            file: PathBuf::from("/tmp/a.jsonl"),
            level: AuditLevel::Matched,
            max_value_len: 1024,
        })
    );
}

#[test]
fn unknown_pattern_names_the_rule_and_pattern() {
    let error = error_text(
        r#"
[[rule]]
decision = "allow"
[[rule]]
decision = "allow"
description = "rm temp"
match."tool_input.command" = { not_regex = "@shell_chian" }
"#,
    );

    assert!(error.contains("rule #2 (rm temp)"), "{error}");
    assert!(error.contains("tool_input.command"), "{error}");
    assert!(error.contains("@shell_chian"), "{error}");
}

#[test]
fn invalid_regex_names_the_rule_and_field() {
    let error = error_text(
        r#"
[[rule]]
decision = "deny"
match."tool_input.file_path" = { regex = '(unclosed' }
"#,
    );

    assert!(error.contains("rule #1"), "{error}");
    assert!(error.contains("tool_input.file_path"), "{error}");
}

#[test]
fn misspelt_matcher_key_names_the_rule_and_key() {
    let error = error_text(
        r#"
[[rule]]
decision = "deny"
match."tool_input.command" = { regx = 'rm' }
"#,
    );

    assert!(error.contains("rule #1"), "{error}");
    assert!(error.contains("regx"), "{error}");
}

#[test]
fn invalid_named_pattern_is_an_error_even_if_unused() {
    let error = error_text("[patterns]\nbroken = '(oops'");

    assert!(error.contains("pattern broken"), "{error}");
}
