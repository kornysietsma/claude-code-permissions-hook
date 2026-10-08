//! Shell commands are checked segment by segment

mod common;

use common::run_with_config;
use pretty_assertions::assert_eq;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tempfile::TempDir;
use tool_gate_hook::{Agent, Context, Outcome, run};

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

[[command_rule]]
decision = "allow"
description = "find"
match.name = { equals = "find" }
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
        (
            "cargo test && if true; then ls; fi",
            UNSUPPORTED,
            "if statement",
        ),
        ("ls; for f in a b; do ls; done", UNSUPPORTED, "for loop"),
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
        let (decision, reason) = decision_allowing_everything(command);
        assert_eq!(decision, "ask", "{command}");
        assert!(reason.starts_with(description), "{command}: {reason}");
        assert!(reason.contains(why), "{command}: {reason}");
    }
}

#[test]
fn floors_only_ask_when_every_segment_is_allowed() {
    for command in [
        "foo $HOME",
        "ls *.rs | foo",
        "for f in a b; do ls; done",
        "FOO=1 foo",
        "FOO=1",
        "bash -c 'ls'",
        "ls 'unterminated",
    ] {
        assert_eq!(decision(command), None, "{command}");
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
    ] {
        let (decision, reason) = decision_allowing_everything(command);
        assert_eq!(decision, "ask", "{command}");
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
        let (decision, reason) = decision_allowing_everything(command);
        assert_eq!(decision, "ask", "{command}");
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
fn a_missing_command_passes_through() {
    let payload =
        r#"{"hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash","tool_input":{}}"#;
    assert_eq!(run_with_config(Agent::Claude, CONFIG, payload).output, None);
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

/// Allows every command, so only floors (and the deny) can stop one
const ALLOW_EVERYTHING: &str = r#"
[[command_rule]]
decision = "allow"
description = "everything"
match.text = { regex = '' }

[[command_rule]]
decision = "deny"
description = "recursive delete"
match.text = { regex = '^rm -rf\b' }
"#;

fn decision_allowing_everything(command: &str) -> (String, String) {
    let output = run_with_config(
        Agent::Claude,
        ALLOW_EVERYTHING,
        &shell_payload(Agent::Claude, command),
    )
    .output
    .unwrap();
    (
        output_field(Agent::Claude, &output, "permissionDecision"),
        output_field(Agent::Claude, &output, "permissionDecisionReason"),
    )
}

const SHELL_REENTRY: &str =
    "tool-gate-hook: ask — a command that runs shell code or changes how later commands run";
const EXEC_TOOL: &str =
    "tool-gate-hook: ask — a command that can run other commands or delete files";

#[test]
fn shells_and_commands_that_run_other_commands_ask_even_when_allowed() {
    for (command, description, detail) in [
        ("bash -c 'ls'", SHELL_REENTRY, "(bash)"),
        ("sh x.sh", SHELL_REENTRY, "(sh)"),
        ("cat x | sh", SHELL_REENTRY, "(sh)"),
        ("/bin/bash x", SHELL_REENTRY, "(bash)"),
        ("env bash x", SHELL_REENTRY, "(bash)"),
        ("eval x", SHELL_REENTRY, "(eval)"),
        ("source x", SHELL_REENTRY, "(source)"),
        (". x", SHELL_REENTRY, "(.)"),
        ("alias ls=x", SHELL_REENTRY, "(alias)"),
        ("setopt x", SHELL_REENTRY, "(setopt)"),
        ("exec cargo test", SHELL_REENTRY, "(exec)"),
        ("ls | xargs rm", EXEC_TOOL, "(xargs)"),
        (r"find . -exec rm {} \;", EXEC_TOOL, "(find -exec)"),
        ("find . -delete", EXEC_TOOL, "(find -delete)"),
    ] {
        let (decision, reason) = decision_allowing_everything(command);
        assert_eq!(decision, "ask", "{command}");
        assert!(reason.starts_with(description), "{command}: {reason}");
        assert!(reason.contains(detail), "{command}: {reason}");
    }
}

#[test]
fn a_deny_beats_shell_reentry_and_cd_floors() {
    assert_eq!(decision_allowing_everything("bash x; rm -rf /").0, "deny");
    assert_eq!(decision_allowing_everything("cd /; rm -rf x").0, "deny");
}

#[test]
fn find_without_actions_can_be_allowed() {
    assert_eq!(decision("find . -name '*.rs'").as_deref(), Some("allow"));
    assert_eq!(decision("find . -delete").as_deref(), Some("ask"));
}

const PATHS_CONFIG: &str = r#"
[[command_rule]]
decision = "allow"
description = "rm in the project"
match.name = { equals = "rm" }
paths_under = ["{cwd}"]

[[command_rule]]
decision = "allow"
description = "cp in the project"
match.name = { equals = "cp" }
paths_under = ["{cwd}"]

[[command_rule]]
decision = "allow"
description = "trash in the scratch directory"
match.name = { equals = "trash" }
paths_under = ["~/scratch"]

[[command_rule]]
decision = "allow"
description = "downloads into the project"
match.text = { regex = '^curl -o ' }
paths_under = ["{cwd}"]

[[command_rule]]
decision = "allow"
description = "project scripts"
match.name = { regex = '^\.', under = ["{cwd}"] }

[[command_rule]]
decision = "allow"
description = "listing"
match.name = { equals = "ls" }
"#;

/// A project with `sub/sub2/` and a `link` to outside it, and a home directory
struct Project {
    root: TempDir,
}

impl Project {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        fs::create_dir_all(root.path().join("project/sub/sub2")).unwrap();
        fs::create_dir_all(root.path().join("home/scratch")).unwrap();
        fs::create_dir_all(root.path().join("outside")).unwrap();
        std::os::unix::fs::symlink(
            root.path().join("outside"),
            root.path().join("project/link"),
        )
        .unwrap();
        Project { root }
    }

    fn decision(&self, command: &str) -> Option<String> {
        self.decision_with(PATHS_CONFIG, command)
            .map(|(decision, _)| decision)
    }

    /// The decision and reason for `command`, run in the project with `config`
    fn decision_with(&self, config: &str, command: &str) -> Option<(String, String)> {
        let config_path = self.root.path().join("claude.toml");
        fs::write(&config_path, config).unwrap();
        let cwd = self.root.path().join("project");
        let payload = Agent::Claude.shell_payload(command, &cwd).to_string();
        let context = Context::new(self.root.path().join("home"));
        run(Agent::Claude, &config_path, &payload, &context)
            .output
            .map(|output| {
                (
                    output_field(Agent::Claude, &output, "permissionDecision"),
                    output_field(Agent::Claude, &output, "permissionDecisionReason"),
                )
            })
    }
}

#[test]
fn paths_under_allows_only_paths_inside() {
    let project = Project::new();
    for (command, expected) in [
        ("rm build/x", Some("allow")),
        ("rm -f ./x sub/y", Some("allow")),
        ("rm /etc/x", None),
        ("rm ../x", None),
        ("rm --out=/etc/x", None),
        ("rm -o/etc/x", None),
        ("cp a b > /etc/x", None),
        ("cp a b > out.txt", Some("allow")),
        ("rm x 2>&1 > /dev/null", Some("allow")),
        ("rm link/x", None),
        ("curl -o page.html https://example.com/x", Some("allow")),
        ("curl -o /etc/page.html https://example.com/x", None),
        ("trash ~/x", None),
        ("trash ~/scratch/x", Some("allow")),
    ] {
        assert_eq!(project.decision(command).as_deref(), expected, "{command}");
    }
}

#[test]
fn cd_into_the_project_is_followed() {
    let project = Project::new();
    for (command, expected) in [
        ("cd sub && rm x", Some("allow")),
        ("cd sub && cd sub2 && rm x", Some("allow")),
        ("cd sub && rm ../../x", None),
        ("cd sub && ./run.sh", Some("allow")),
        ("cd sub && ../../evil.sh", None),
        ("cd sub", None),
        ("pushd sub && ls", Some("allow")),
    ] {
        assert_eq!(project.decision(command).as_deref(), expected, "{command}");
    }
}

#[test]
fn a_cd_that_may_not_have_happened_is_not_trusted() {
    let project = Project::new();
    // The shell may still be in the project root: cd can fail, and a subshell's cd ends with it
    for command in [
        "cd sub && rm ../x",
        "cd missing; rm ../x",
        "(cd sub); rm ../x",
        "cd sub || rm ../x",
        "echo $(cd sub) && rm ../x",
    ] {
        assert_ne!(
            project.decision(command).as_deref(),
            Some("allow"),
            "{command}"
        );
    }
}

#[test]
fn other_directory_changes_ask() {
    let project = Project::new();
    for command in [
        "cd /tmp && ls",
        "cd && ls",
        "cd - && ls",
        "cd -P sub && ls",
        "cd ../ && ls",
        "cd link && ls",
        "cd $D && ls",
        "cd a b && ls",
        "popd && ls",
        "env cd sub && ls",
    ] {
        let decided = project.decision_with(ALLOW_EVERYTHING, command);
        assert_eq!(decided.unwrap().0, "ask", "{command}");
    }
}

const CONSTRUCTS_CONFIG: &str = r#"
[[command_rule]]
decision = "allow"
description = "cargo test"
match.text = { regex = '^cargo test\b' }

[[command_rule]]
decision = "deny"
description = "recursive delete"
match.text = { regex = '^rm -rf\b' }

[[construct_rule]]
decision = "ask"
construct = "background"
description = "backgrounded commands"

[[construct_rule]]
decision = "ask"
construct = "redirect_write"
description = "writes outside the project"
outside = ["{cwd}", "/tmp"]
"#;

fn construct_decision(project: &Project, command: &str) -> Option<String> {
    project
        .decision_with(CONSTRUCTS_CONFIG, command)
        .map(|(decision, _)| decision)
}

#[test]
fn writes_outside_the_listed_directories_ask() {
    let project = Project::new();
    for (command, expected) in [
        ("cargo test > out.txt", "allow"),
        ("cargo test > sub/out.txt", "allow"),
        ("cargo test > /tmp/out.txt", "allow"),
        ("cargo test 2>&1 > /dev/null", "allow"),
        ("cargo test 2>&1 >&- 3>&2-", "allow"),
        ("cargo test < /etc/x", "allow"),
        ("cargo test > ~/out.txt", "ask"),
        ("cargo test > /etc/x", "ask"),
        ("cargo test >> /etc/x", "ask"),
        ("cargo test 2> /etc/x", "ask"),
        ("cargo test &> /etc/x", "ask"),
        ("cargo test >& /etc/x", "ask"),
        ("cargo test >| /etc/x", "ask"),
        ("cargo test <> /etc/x", "ask"),
        ("cargo test > link/x", "ask"),
        ("cargo test > ../x", "ask"),
    ] {
        assert_eq!(
            construct_decision(&project, command).as_deref(),
            Some(expected),
            "{command}"
        );
    }
}

#[test]
fn redirect_targets_are_checked_from_every_possible_directory() {
    let project = Project::new();
    for (command, expected) in [
        ("cd sub && cargo test > x", "allow"),
        ("cd sub && cargo test > ../../x", "ask"),
        // From the project root, where the shell may still be, ../x is outside
        ("cd sub && cargo test > ../x", "ask"),
        // A group's own redirect is opened before the group runs
        ("(cd sub && cargo test) > x", "allow"),
        ("(cd sub && cargo test) > ../x", "ask"),
        ("cd sub && (cargo test) > ../x", "ask"),
    ] {
        assert_eq!(
            construct_decision(&project, command).as_deref(),
            Some(expected),
            "{command}"
        );
    }
}

#[test]
fn construct_rules_ask_even_when_every_segment_is_allowed() {
    let project = Project::new();
    assert_eq!(
        project.decision_with(CONSTRUCTS_CONFIG, "cargo test &"),
        Some((
            "ask".to_owned(),
            "tool-gate-hook: ask by construct rule #1 (backgrounded commands) — in \"cargo test\""
                .to_owned()
        ))
    );
    // The first target outside the directories is named
    assert_eq!(
        project
            .decision_with(CONSTRUCTS_CONFIG, "cargo test > x 2> /etc/x > /etc/y")
            .map(|(_, reason)| reason)
            .as_deref(),
        Some("tool-gate-hook: ask by construct rule #2 (writes outside the project) — \"/etc/x\"")
    );
    // Commands no rule matched ask too, rather than passing through
    assert_eq!(
        construct_decision(&project, "make &").as_deref(),
        Some("ask")
    );
    assert_eq!(
        construct_decision(&project, "cargo test && rm -rf x &").as_deref(),
        Some("deny")
    );
}

#[test]
fn a_construct_rule_can_deny() {
    let project = Project::new();
    let config = format!(
        "{CONSTRUCTS_CONFIG}\n[[construct_rule]]\ndecision = \"deny\"\nconstruct = \"pipe\"\nreason = \"No pipes\""
    );
    assert_eq!(
        project.decision_with(&config, "cargo test | cargo test &"),
        Some(("deny".to_owned(), "No pipes — in \"cargo test\"".to_owned()))
    );
}

#[test]
fn construct_rule_config_errors() {
    for (rule, expected) in [
        (
            "decision = \"allow\"\nconstruct = \"pipe\"",
            "construct rule #1: a construct rule can only ask or deny, not allow",
        ),
        (
            "decision = \"ask\"\nconstruct = \"pipes\"",
            "construct rule #1: unknown construct \"pipes\" (expected one of parse_error, \
             unsupported, expansion, dynamic_command, env_assign, shell_reentry, exec_tool, cd, \
             substitution, heredoc, pipe, background, subshell, redirect_read, redirect_write)",
        ),
        (
            "decision = \"ask\"\nconstruct = \"pipe\"\noutside = [\"{cwd}\"]",
            "construct rule #1: outside is only for redirect_write",
        ),
        (
            "decision = \"ask\"\nconstruct = \"redirect_write\"\noutside = []",
            "construct rule #1: outside: empty list",
        ),
        ("decision = \"ask\"", "missing field `construct`"),
        (
            "decision = \"ask\"\nconstruct = \"pipe\"\nmatch.text = { regex = 'x' }",
            "unknown field `match`",
        ),
    ] {
        let error = config_error(Agent::Claude, &format!("[[construct_rule]]\n{rule}"));
        assert!(error.contains(expected), "{rule}: {error}");
    }
}

const FLOOR_RULES_CONFIG: &str = r#"
[[construct_rule]]
decision = "ask"
construct = "shell_reentry"
description = "runs shell code"

[[construct_rule]]
decision = "deny"
construct = "exec_tool"
description = "runs other commands"

[[construct_rule]]
decision = "ask"
construct = "unsupported"
description = "unchecked syntax"
"#;

#[test]
fn construct_rules_can_ask_or_deny_on_floors_nothing_allows() {
    let project = Project::new();
    for (command, expected) in [
        (
            "bash -c 'pip install x'",
            Some((
                "ask",
                "tool-gate-hook: ask by construct rule #1 (runs shell code) — in \"bash -c 'pip install x'\"",
            )),
        ),
        (
            "echo x | xargs pip install",
            Some((
                "deny",
                "tool-gate-hook: deny by construct rule #2 (runs other commands) — in \"xargs pip install\"",
            )),
        ),
        (
            "for f in a; do pip install x; done",
            Some((
                "ask",
                "tool-gate-hook: ask by construct rule #3 (unchecked syntax) — for loop",
            )),
        ),
        ("ls $HOME", None),
    ] {
        let decided = project.decision_with(FLOOR_RULES_CONFIG, command);
        assert_eq!(
            decided.as_ref().map(|(d, r)| (d.as_str(), r.as_str())),
            expected,
            "{command}"
        );
    }
}

#[test]
fn a_rule_that_asks_is_the_reason_rather_than_a_floor() {
    assert_eq!(
        reason("git push $REMOTE"),
        "tool-gate-hook: ask by command rule #6 (git push) — in \"git push '$REMOTE'\""
    );
    let project = Project::new();
    let (_, reason) = project
        .decision_with(CONSTRUCTS_CONFIG, "foo *.md > /etc/x")
        .unwrap();
    assert!(reason.contains("by construct rule"), "{reason}");
}

#[test]
fn reasons_quote_only_the_first_line_and_skip_repeats() {
    assert_eq!(
        decision_allowing_everything("cat x | sh").1,
        "tool-gate-hook: ask — a command that runs shell code or changes how later commands run (sh)"
    );
    assert_eq!(
        decision_allowing_everything("cat <<EOF > x\nline $HOME\nEOF").1,
        "tool-gate-hook: ask — a value only the shell can work out (variable in heredoc body), \
         in \"cat <<EOF …\""
    );
    let long = format!("ls {}", "a".repeat(200));
    assert_eq!(
        reason(&format!("{long} && git push")),
        "tool-gate-hook: ask by command rule #6 (git push) — in \"git push\""
    );
    let (_, reason) = decision_allowing_everything(&format!("{long} $HOME"));
    assert!(
        reason.ends_with(&format!("in \"{} …\"", &long[..100])),
        "{reason}"
    );
}
