//! Our word splitting must agree with the shells that actually run the commands: zsh (Claude
//! Code on macOS) and bash (Copilot). Each case is run with its command word replaced by a
//! function that prints its arguments; a shell that isn't installed is skipped.

use pretty_assertions::assert_eq;
use std::path::Path;
use std::process::Command;
use tool_gate_hook::shell::analyse;

const HOME: &str = "/home/me";

/// Static simple commands that zsh, bash and we must split identically
const AGREED: &[&str] = &[
    r#"echo a"b"'c'"#,
    r#"echo "x\"y" 'a\b' "a\b" a\ b"#,
    r#"echo $'tab\there' $'\x41\101' $'it\'s'"#,
    "echo a\\\nb",
    "echo hi # a comment",
    r##"echo a#b "#c""##,
    r#"echo '' "" x"#,
    r#"echo "it's" 'say "hi"'"#,
    "git log --format='%H %s' -- src/",
    r#"echo \\ \' \" \$x"#,
    "echo HEAD@{1} {} [ ]",
    "echo ~/x ~ a~b",
    "echo = a=b --opt=1",
    r#"echo "a\$b" "\`" "a\b\c""#,
    "cargo test --all -- --nocapture",
    "git commit -m 'Fix: handle \"quoted\" (and) [brackets]; done'",
];

/// Words zsh and bash may expand differently; we must not treat them as static
const DISAGREED: &[&str] = &["echo =ls", "echo *.rs", "echo {a,b}", "echo ${(f)x}"];

fn shell_words(shell: &str, args: &[&str], command: &str) -> Option<Vec<String>> {
    if !Path::new(shell).exists() {
        eprintln!("skipping {shell}: not installed");
        return None;
    }
    let (_, rest) = command.split_once(' ').unwrap_or((command, ""));
    let script = format!("__d() {{ printf '%s\\0' \"$@\"; }}; __d {rest}");
    let output = Command::new(shell)
        .args(args)
        .arg(&script)
        .env("HOME", HOME)
        .output()
        .unwrap();
    assert!(output.status.success(), "{shell}: {command}: {output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    Some(stdout.split_terminator('\0').map(str::to_owned).collect())
}

fn our_args(command: &str) -> Vec<String> {
    let analysis = analyse(command, Path::new(HOME));
    assert_eq!(analysis.constructs, vec![], "{command}");
    assert_eq!(analysis.segments.len(), 1, "{command}");
    analysis.segments[0].args.clone()
}

#[test]
fn zsh_and_bash_split_words_as_we_do() {
    for command in AGREED {
        let ours = our_args(command);
        for (shell, args) in [("/bin/zsh", &["-f", "-c"][..]), ("/bin/bash", &["-c"][..])] {
            if let Some(theirs) = shell_words(shell, args, command) {
                assert_eq!(ours, theirs, "{shell}: {command}");
            }
        }
    }
}

#[test]
fn words_the_shells_may_expand_are_never_static() {
    for command in DISAGREED {
        let analysis = analyse(command, Path::new(HOME));
        assert!(
            analysis.constructs.iter().any(|c| c.kind.is_floor()),
            "{command}"
        );
    }
}
