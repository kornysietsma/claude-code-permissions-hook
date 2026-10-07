use pretty_assertions::assert_eq;
use std::path::PathBuf;
use tool_gate_hook::Agent;
use tool_gate_hook::config::{AuditConfig, AuditLevel, Config};
use tool_gate_hook::policy::{Decision, FieldMatcher};
use tool_gate_hook::validate::validate;

const VALID: &str = r#"
[audit]
file = "/tmp/audit.jsonl"
level = "all"

[patterns]
shell_chain = ';|&&|\|'

[[rule]]
decision = "ask"
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
        Config::from_toml(toml, Agent::Claude).expect_err("config should be invalid")
    )
}

#[test]
fn valid_config_compiles_rules_in_file_order() {
    let config = Config::from_toml(VALID, Agent::Claude).unwrap();

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
        vec![(1, Decision::Ask), (2, Decision::Deny), (3, Decision::Ask)]
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
    let config = Config::from_toml(VALID, Agent::Claude).unwrap();
    let tool = config.policy.rules[0].tool.as_ref().unwrap();

    assert!(tool.is_match("Bash"));
    assert!(!tool.is_match("BashOutput"));
}

#[test]
fn audit_section_is_optional_and_defaults_apply() {
    assert_eq!(Config::from_toml("", Agent::Claude).unwrap().audit, None);

    let config = Config::from_toml("[audit]\nfile = \"/tmp/a.jsonl\"", Agent::Claude).unwrap();
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
decision = "deny"
[[rule]]
decision = "deny"
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

#[test]
fn shell_safe_env_takes_regexes_and_patterns() {
    let config = Config::from_toml(
        "[patterns]\nrust = '^RUST_LOG$'\n\n[shell]\nsafe_env = ['@rust', '^CI$']",
        Agent::Claude,
    )
    .unwrap();

    assert_eq!(config.policy.shell.safe_env.len(), 2);
    assert_eq!(config.policy.shell.safe_env[0].as_str(), "^RUST_LOG$");
}

#[test]
fn shell_section_errors() {
    for (toml, expected) in [
        ("[shell]\nsafe_envs = []", "unknown field `safe_envs`"),
        ("[shell]\nsafe_env = ['(oops']", "[shell] safe_env"),
        (
            "[shell]\nsafe_env = ['@missing']",
            "unknown pattern @missing",
        ),
    ] {
        let error = error_text(toml);
        assert!(error.contains(expected), "{toml}: {error}");
    }
}

#[test]
fn paths_under_alone_is_enough_for_a_command_rule() {
    let config = Config::from_toml(
        "[[command_rule]]\ndecision = \"allow\"\npaths_under = [\"{cwd}\"]",
        Agent::Claude,
    )
    .unwrap();

    assert_eq!(config.policy.command_rules[0].paths_under, vec!["{cwd}"]);
}

#[test]
fn paths_under_must_list_directories() {
    let error = error_text("[[command_rule]]\ndecision = \"allow\"\npaths_under = []");

    assert!(
        error.contains("command rule #1: paths_under: empty list"),
        "{error}"
    );
}

#[test]
fn paths_under_is_only_for_command_rules() {
    let error = error_text("[[rule]]\ndecision = \"ask\"\npaths_under = [\"{cwd}\"]");

    assert!(error.contains("paths_under"), "{error}");
}

#[test]
fn validate_warns_about_command_rule_fields_that_are_not_segment_fields() {
    let config = Config::from_toml(
        r#"
[patterns]
cargo = '^cargo\b'

[[command_rule]]
decision = "allow"
description = "copied from a [[rule]]"
match."tool_input.command" = { regex = '^cargo\b' }

[[command_rule]]
decision = "allow"
match.text = { regex = '@cargo' }
match."env.RUST_LOG" = { exists = true }
"#,
        Agent::Claude,
    )
    .unwrap();

    assert_eq!(
        validate(Agent::Claude, &config).warnings,
        vec![
            "command rule #1 (copied from a [[rule]]): field \"tool_input.command\" does not \
             start with a segment field (text, name, args, env, redirects, wrappers, source, dirs)"
                .to_owned()
        ]
    );
}

#[test]
fn validate_summary_lists_construct_rules_and_safe_env() {
    let config = Config::from_toml(
        r#"
[patterns]
rust = '^RUST_LOG$'

[shell]
safe_env = ['@rust', '^CI$']

[[construct_rule]]
decision = "deny"
construct = "pipe"

[[construct_rule]]
decision = "ask"
construct = "redirect_write"
outside = ["{cwd}"]
"#,
        Agent::Claude,
    )
    .unwrap();

    assert_eq!(
        validate(Agent::Claude, &config).summary,
        "0 rules (allow 0, ask 0, deny 0)\n\
         0 command rules (allow 0, ask 0, deny 0)\n\
         2 construct rules (deny on pipe; ask on redirect_write outside {cwd})\n\
         safe_env: '^RUST_LOG$', '^CI$'\n\
         patterns: rust\n\
         audit: off (no [audit] section)"
    );
}
