//! Shell commands are checked segment by segment

mod common;

use common::run_with_config;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::path::Path;
use tool_gate_hook::{Agent, Outcome};

fn shell_payload(agent: Agent, command: &str) -> String {
    agent.shell_payload(command, Path::new("/tmp")).to_string()
}

const CONFIG: &str = r#"
[patterns]
rust_env = '^RUST_(LOG|BACKTRACE)$'

[shell]
safe_env = ['@rust_env', '^NO_COLOR$']

[[command_rule]]
decision = "allow"
description = "cargo workflow"
match.text = { regex = '^cargo (build|test|check)\b' }

[[command_rule]]
decision = "allow"
description = "git commit"
match.text = { regex = '^git commit\b' }

[[command_rule]]
decision = "allow"
description = "listing"
match.name = { equals = "ls" }

[[command_rule]]
decision = "deny"
description = "recursive delete"
reason = "No recursive deletes"
match.text = { regex = '^rm -rf\b' }

[[command_rule]]
decision = "deny"
description = "legacy Python"
match.name = { regex = '(^|/)(python[0-9.]*|pip[0-9]*)$' }

[[command_rule]]
decision = "ask"
description = "git push"
match.text = { regex = '^git push\b' }

[[command_rule]]
decision = "allow"
description = "harmless"
match.name = { regex = '^(echo|cat|date)$' }

[[command_rule]]
decision = "deny"
description = "downloads"
match.name = { equals = "curl" }

[[command_rule]]
decision = "allow"
description = "read-only git"
match.text = { regex = '^git (log|status)\b' }
"#;

fn outcome(agent: Agent, command: &str) -> Outcome {
    run_with_config(agent, CONFIG, &shell_payload(agent, command))
}

fn output_field(agent: Agent, output: &Value, field: &str) -> String {
    let decision = match agent {
        Agent::Claude => &output["hookSpecificOutput"],
        Agent::Copilot => output,
    };
    decision[field].as_str().unwrap().to_owned()
}

/// The decision for `command` with both agents, which must agree
fn decision(command: &str) -> Option<String> {
    let [claude, copilot] = [Agent::Claude, Agent::Copilot].map(|agent| {
        outcome(agent, command)
            .output
            .map(|output| output_field(agent, &output, "permissionDecision"))
    });
    assert_eq!(claude, copilot, "{command}");
    claude
}

fn reason(command: &str) -> String {
    let output = outcome(Agent::Claude, command).output.unwrap();
    output_field(Agent::Claude, &output, "permissionDecisionReason")
}

#[test]
fn a_compound_command_is_allowed_when_every_segment_is() {
    for command in [
        "cargo build 2>&1 && cargo test",
        "cargo check; cargo test --all | ls",
        "( cargo build ) && { cargo test; }",
        "time cargo test",
        "cargo test\nls -la",
    ] {
        assert_eq!(decision(command).as_deref(), Some("allow"), "{command}");
    }
}

#[test]
fn a_segment_no_rule_matches_passes_the_whole_command_through() {
    assert_eq!(decision("foo && cargo test"), None);
    assert_eq!(decision("cargo test | grep ok"), None);
}

#[test]
fn a_denied_segment_denies_the_whole_command() {
    assert_eq!(decision("cargo test; rm -rf /").as_deref(), Some("deny"));
    assert_eq!(decision("ls && python3 x.py").as_deref(), Some("deny"));
    assert_eq!(
        reason("cargo test; rm -rf /"),
        "No recursive deletes — in \"rm -rf /\""
    );
}

#[test]
fn an_asked_segment_asks() {
    assert_eq!(decision("cargo test && git push").as_deref(), Some("ask"));
    assert_eq!(
        reason("cargo test && git push"),
        "tool-gate-hook: ask by command rule #6 (git push) — in \"git push\""
    );
}

#[test]
fn quoting_is_understood() {
    assert_eq!(
        decision("git commit -F - <<'EOF'\nTidy up\n\npip install is no longer needed\nEOF")
            .as_deref(),
        Some("allow")
    );
    assert_eq!(
        decision("git commit -m 'stop using python3; use uv'").as_deref(),
        Some("allow")
    );
    // Quoted operators are just text: one `ls` segment, so no deny
    assert_eq!(decision("ls 'a && rm -rf /'").as_deref(), Some("allow"));
}

const UNSUPPORTED: &str = "tool-gate-hook: ask — shell syntax that tool-gate-hook can't check";
const EXPANSION: &str = "tool-gate-hook: ask — a value only the shell can work out";
const DYNAMIC_COMMAND: &str = "tool-gate-hook: ask — a command name only the shell can work out";

#[test]
fn syntax_that_cannot_be_checked_asks_even_when_every_segment_is_allowed() {
    for (command, description, why) in [
        ("if true; then cargo test; fi", UNSUPPORTED, "if statement"),
        ("for f in a b; do ls; done", UNSUPPORTED, "for loop"),
        ("ls $HOME", EXPANSION, "variable in $HOME"),
        ("ls *.rs", EXPANSION, "glob in *.rs"),
        ("ls ${(f)x}", EXPANSION, "variable in ${(f)x}"),
        ("echo {a,b}", EXPANSION, "brace expansion in {a,b}"),
        ("echo \"$HOME\"", EXPANSION, "variable in \"$HOME\""),
        ("echo x > $f", EXPANSION, "variable in redirect $f"),
        ("cat <<< $x", EXPANSION, "variable in here-string $x"),
        (
            "cargo test --features $(cat f)",
            EXPANSION,
            "command substitution in $(cat f)",
        ),
        ("echo `date`", EXPANSION, "command substitution in `date`"),
        (
            "cat <<EOF\nhi $USER\nEOF",
            EXPANSION,
            "variable in heredoc body",
        ),
        ("$CMD x", DYNAMIC_COMMAND, "variable in $CMD"),
        (
            "=python3 x.py",
            DYNAMIC_COMMAND,
            "zsh =command expansion in =python3",
        ),
    ] {
        assert_eq!(decision(command).as_deref(), Some("ask"), "{command}");
        let reason = reason(command);
        assert!(reason.starts_with(description), "{command}: {reason}");
        assert!(reason.contains(why), "{command}: {reason}");
    }
}

const ENV_ASSIGN: &str = "tool-gate-hook: ask — a variable not listed in [shell] safe_env";

#[test]
fn safe_env_assignments_are_allowed() {
    for command in [
        "RUST_LOG=debug cargo test",
        "env RUST_LOG=debug NO_COLOR=1 cargo test",
        "RUST_BACKTRACE=1; cargo test",
    ] {
        assert_eq!(decision(command).as_deref(), Some("allow"), "{command}");
    }
    // `export` is an ordinary command, and CONFIG has no rule for it
    assert_eq!(decision("export RUST_LOG=debug && cargo test"), None);
}

#[test]
fn other_assignments_ask() {
    for (command, name) in [
        ("GIT_PAGER=x git log", "GIT_PAGER"),
        ("PATH=./evil cargo test", "PATH"),
        ("PATH=./evil; cargo test", "PATH"),
        ("export GIT_PAGER=x; git log", "GIT_PAGER"),
        ("declare -x GIT_PAGER=x; git log", "GIT_PAGER"),
        ("env GIT_PAGER=x git log", "GIT_PAGER"),
        ("FOO=1", "FOO"),
    ] {
        assert_eq!(decision(command).as_deref(), Some("ask"), "{command}");
        let reason = reason(command);
        assert!(reason.starts_with(ENV_ASSIGN), "{command}: {reason}");
        assert!(
            reason.contains(&format!("assignment {name}")),
            "{command}: {reason}"
        );
    }
}

#[test]
fn a_command_of_only_safe_assignments_passes_through() {
    assert_eq!(decision("RUST_LOG=1"), None);
}

#[test]
fn assignment_values_are_checked() {
    assert_eq!(decision("RUST_LOG=$x cargo test").as_deref(), Some("ask"));
    assert!(reason("RUST_LOG=$x cargo test").contains("variable in RUST_LOG=$x"));
    assert_eq!(decision("X=$(curl x)").as_deref(), Some("deny"));
    assert_eq!(
        decision("RUST_LOG=$(curl x) cargo test").as_deref(),
        Some("deny")
    );
}

#[test]
fn a_deny_beats_an_assignment_floor() {
    assert_eq!(decision("PATH=./evil rm -rf /").as_deref(), Some("deny"));
}

#[test]
fn wrapped_commands_are_checked() {
    for command in [
        "timeout 60 cargo test",
        "timeout -s KILL 60 cargo test",
        "nice -n 5 cargo test",
        "nohup cargo test",
        "env RUST_LOG=debug timeout 60 cargo test",
    ] {
        assert_eq!(decision(command).as_deref(), Some("allow"), "{command}");
    }
    assert_eq!(decision("timeout 60 python3 x.py").as_deref(), Some("deny"));
    assert_eq!(decision("nice env rm -rf /").as_deref(), Some("deny"));
    // A wrapper with nothing to run is the command itself
    assert_eq!(decision("timeout"), None);
}

#[test]
fn unknown_wrapper_options_ask() {
    for (command, why) in [
        ("timeout --bogus 60 cargo test", "timeout option --bogus"),
        ("env -i cargo test", "env option -i"),
        ("nice -5 cargo test", "nice option -5"),
    ] {
        assert_eq!(decision(command).as_deref(), Some("ask"), "{command}");
        let reason = reason(command);
        assert!(reason.starts_with(UNSUPPORTED), "{command}: {reason}");
        assert!(reason.contains(why), "{command}: {reason}");
    }
}

#[test]
fn quoted_words_are_static() {
    for command in [
        "ls '*.rs'",
        "echo '$HOME'",
        "echo '{a,b}'",
        "cat <<'EOF'\n$(curl x) $HOME\nEOF",
        "cat <<\"EOF\"\n`curl x`\nEOF",
        "cat <<< 'hi there'",
    ] {
        assert_eq!(decision(command).as_deref(), Some("allow"), "{command}");
    }
}

#[test]
fn equals_signs_are_only_an_expansion_when_they_could_name_a_command() {
    // zsh fails on `====` (no command is called `===`), so it can never run anything hidden
    assert_eq!(decision("echo ==== && echo =-=-").as_deref(), Some("allow"));
    assert_eq!(decision("echo =ls").as_deref(), Some("ask"));
}

#[test]
fn a_deny_beats_a_floor() {
    assert_eq!(decision("rm -rf $HOME").as_deref(), Some("deny"));
    assert_eq!(decision("$CMD; rm -rf /").as_deref(), Some("deny"));
}

#[test]
fn substituted_commands_are_checked_too() {
    for command in [
        "echo $(curl x)",
        "echo \"$(curl x)\"",
        "echo `curl x`",
        "echo $(echo $(curl x))",
        "diff <(curl x) b",
        "ls > >(curl x)",
        "cat <<EOF\n$(curl x)\nEOF",
        "cat <<EOF\n`curl x`\nEOF",
        "cat <<< $(curl x)",
        "{ ls; } > $(curl x)",
    ] {
        assert_eq!(decision(command).as_deref(), Some("deny"), "{command}");
    }
    assert_eq!(
        reason("echo $(curl x)"),
        "tool-gate-hook: deny by command rule #8 (downloads) — in \"curl x\""
    );
}

#[test]
fn substitutions_ask_even_when_every_command_in_them_is_allowed() {
    for command in [
        "echo $(date)",
        "cat <<EOF\n$(date)\nEOF",
        "cat <(ls a) <(ls b)",
        "ls > >(cat)",
    ] {
        assert_eq!(decision(command).as_deref(), Some("ask"), "{command}");
    }
}

#[test]
fn an_unparseable_command_asks() {
    assert_eq!(decision("ls 'unterminated").as_deref(), Some("ask"));
    assert!(
        reason("ls 'unterminated")
            .starts_with("tool-gate-hook: ask — the command could not be parsed")
    );
}

#[test]
fn a_missing_command_asks() {
    let payload =
        r#"{"hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash","tool_input":{}}"#;
    let output = run_with_config(Agent::Claude, CONFIG, payload)
        .output
        .unwrap();

    assert_eq!(
        output_field(Agent::Claude, &output, "permissionDecision"),
        "ask"
    );
    assert!(
        output_field(Agent::Claude, &output, "permissionDecisionReason")
            .contains("no command string")
    );
}

#[test]
fn whole_payload_rules_can_still_deny_or_ask_shell_calls() {
    let config = format!(
        "{CONFIG}\n[[rule]]\ndecision = \"ask\"\ntool = \"Bash\"\nmatch.\"agent_id\" = {{ exists = true }}"
    );
    let mut payload: Value = serde_json::from_str(&shell_payload(Agent::Claude, "ls")).unwrap();
    payload["agent_id"] = "subagent-1".into();
    let output = run_with_config(Agent::Claude, &config, &payload.to_string())
        .output
        .unwrap();

    assert_eq!(
        output_field(Agent::Claude, &output, "permissionDecision"),
        "ask"
    );
}

#[test]
fn copilot_hooks_registered_in_claude_settings_get_shell_rules_too() {
    let stdin = common::fixture("copilot_via_claude", "bash");
    let output = run_with_config(Agent::Claude, CONFIG, &stdin)
        .output
        .unwrap();

    assert_eq!(
        output_field(Agent::Claude, &output, "permissionDecision"),
        "allow"
    );
}

fn config_error(agent: Agent, config: &str) -> String {
    let outcome = run_with_config(agent, config, &shell_payload(agent, "ls"));
    let output = outcome.output.unwrap();
    assert_eq!(output_field(agent, &output, "permissionDecision"), "ask");
    output_field(agent, &output, "permissionDecisionReason")
}

#[test]
fn an_allow_rule_that_could_match_the_shell_tool_is_a_config_error() {
    for (agent, rule) in [
        (
            Agent::Claude,
            "[[rule]]\ndecision = \"allow\"\ntool = \"Bash\"",
        ),
        (
            Agent::Claude,
            "[[rule]]\ndecision = \"allow\"\ntool = \"Read|Bash\"",
        ),
        (Agent::Claude, "[[rule]]\ndecision = \"allow\""),
        (
            Agent::Copilot,
            "[[rule]]\ndecision = \"allow\"\ntool = \"bash\"",
        ),
    ] {
        let error = config_error(agent, rule);
        assert!(
            error.contains("rule #1: an allow [[rule]] can't match the shell tool"),
            "{error}"
        );
    }
}

#[test]
fn a_command_rule_needs_conditions_and_known_keys() {
    let empty = config_error(Agent::Claude, "[[command_rule]]\ndecision = \"allow\"");
    assert!(
        empty.contains("command rule #1: no match conditions"),
        "{empty}"
    );

    let typo = config_error(
        Agent::Claude,
        "[[command_rule]]\ndecision = \"allow\"\ntool = \"Bash\"\nmatch.text = { regex = 'x' }",
    );
    assert!(typo.contains("command rule #1"), "{typo}");
    assert!(typo.contains("tool"), "{typo}");
}
