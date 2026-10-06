//! Commands that run another command (`env`, `timeout`, `nice`, `nohup`, `time`), unwrapped so
//! rules see the command they run

use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Unwrapped {
    /// The wrapper names, outermost first
    pub wrappers: Vec<String>,
    /// `NAME=value` words given to `env`, in order
    pub env: Vec<(String, String)>,
    pub name: String,
    pub args: Vec<String>,
}

/// Unwraps every wrapper around `name`. Only bare names are wrappers: `./env` could be anything.
/// A wrapper with no command after it is left as the command. `Err` describes an option we
/// don't understand.
pub fn unwrap(name: &str, args: &[String]) -> Result<Unwrapped, String> {
    let mut unwrapped = Unwrapped {
        name: name.to_owned(),
        args: args.to_vec(),
        ..Unwrapped::default()
    };
    loop {
        let skip = match unwrapped.name.as_str() {
            "env" => env_options(&unwrapped.args, &mut unwrapped.env)?,
            "timeout" => timeout_options(&unwrapped.args)?,
            "nice" => nice_options(&unwrapped.args)?,
            "nohup" => no_options("nohup", &unwrapped.args, &[])?,
            "time" => no_options("time", &unwrapped.args, &["-p"])?,
            _ => return Ok(unwrapped),
        };
        let Some((name, args)) = unwrapped.args.split_at_checked(skip).and_then(|(_, rest)| {
            let (name, args) = rest.split_first()?;
            Some((name.clone(), args.to_vec()))
        }) else {
            return Ok(unwrapped);
        };
        unwrapped
            .wrappers
            .push(std::mem::replace(&mut unwrapped.name, name));
        unwrapped.args = args;
    }
}

/// Each `env_options` style function returns how many args belong to the wrapper
fn env_options(args: &[String], env: &mut Vec<(String, String)>) -> Result<usize, String> {
    let mut assignments = vec![];
    for arg in args {
        if arg.starts_with('-') {
            return Err(format!("env option {arg}"));
        }
        let Some((name, value)) = arg.split_once('=') else {
            break;
        };
        assignments.push((name.to_owned(), value.to_owned()));
    }
    let count = assignments.len();
    // `env FOO=1` with no command just prints the environment
    if count < args.len() {
        env.extend(assignments);
    }
    Ok(count)
}

fn timeout_options(args: &[String]) -> Result<usize, String> {
    static DURATION: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^([0-9]+\.?[0-9]*|\.[0-9]+)[smhd]?$").unwrap());
    let mut i = 0;
    while let Some(arg) = args.get(i) {
        match arg.as_str() {
            "-s" | "-k" => i += 2,
            "--preserve-status" | "--foreground" | "-v" => i += 1,
            _ if arg.starts_with("--signal=") || arg.starts_with("--kill-after=") => i += 1,
            _ if arg.starts_with('-') => return Err(format!("timeout option {arg}")),
            _ if DURATION.is_match(arg) => return Ok(i + 1),
            _ => return Err(format!("timeout duration {arg}")),
        }
    }
    Ok(i)
}

fn nice_options(args: &[String]) -> Result<usize, String> {
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+-]?[0-9]+$").unwrap());
    let Some(first) = args.first() else {
        return Ok(0);
    };
    let (adjustment, count) = match first.as_str() {
        "-n" => (args.get(1).map(String::as_str), 2),
        _ if first.starts_with("--adjustment=") => (first.strip_prefix("--adjustment="), 1),
        _ if first.starts_with("-n") => (first.strip_prefix("-n"), 1),
        _ if first.starts_with('-') => return Err(format!("nice option {first}")),
        _ => return Ok(0),
    };
    match adjustment {
        Some(n) if !NUMBER.is_match(n) => Err(format!("nice adjustment {n}")),
        _ => Ok(count),
    }
}

fn no_options(wrapper: &str, args: &[String], allowed: &[&str]) -> Result<usize, String> {
    let options = args
        .iter()
        .take_while(|arg| allowed.contains(&arg.as_str()))
        .count();
    match args.get(options) {
        Some(arg) if arg.starts_with('-') => Err(format!("{wrapper} option {arg}")),
        _ => Ok(options),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn unwrap_words(command: &str) -> Result<Unwrapped, String> {
        let words: Vec<String> = command.split(' ').map(str::to_owned).collect();
        unwrap(&words[0], &words[1..])
    }

    /// wrappers, then the command's words
    fn unwrapped(command: &str) -> (Vec<String>, String) {
        let result = unwrap_words(command).unwrap();
        let words = std::iter::once(result.name)
            .chain(result.args)
            .collect::<Vec<_>>();
        (result.wrappers, words.join(" "))
    }

    fn error(command: &str) -> String {
        unwrap_words(command).unwrap_err()
    }

    #[test]
    fn other_commands_are_left_alone() {
        assert_eq!(unwrapped("cargo test"), (vec![], "cargo test".into()));
        assert_eq!(unwrapped("./env cargo"), (vec![], "./env cargo".into()));
        assert_eq!(
            unwrapped("/usr/bin/env cargo"),
            (vec![], "/usr/bin/env cargo".into())
        );
    }

    #[test]
    fn env_takes_assignments() {
        let result = unwrap_words("env A=1 B=x=y cargo test").unwrap();

        assert_eq!(result.wrappers, vec!["env"]);
        assert_eq!(
            result.env,
            vec![("A".into(), "1".into()), ("B".into(), "x=y".into())]
        );
        assert_eq!(
            (result.name.as_str(), result.args),
            ("cargo", vec!["test".to_owned()])
        );
        assert_eq!(error("env -i cargo"), "env option -i");
        assert_eq!(error("env -- cargo"), "env option --");
        assert_eq!(error("env A=1 -u B cargo"), "env option -u");
    }

    #[test]
    fn timeout_takes_options_and_a_duration() {
        for command in [
            "timeout 60 cargo test",
            "timeout 1.5m cargo test",
            "timeout -s KILL 60 cargo test",
            "timeout --signal=KILL -k 5 --kill-after=5s 60 cargo test",
            "timeout --preserve-status --foreground -v 60 cargo test",
        ] {
            assert_eq!(
                unwrapped(command),
                (vec!["timeout".into()], "cargo test".into()),
                "{command}"
            );
        }
        assert_eq!(error("timeout --bogus 60 cargo"), "timeout option --bogus");
        assert_eq!(error("timeout -sKILL 60 cargo"), "timeout option -sKILL");
        assert_eq!(error("timeout cargo test"), "timeout duration cargo");
    }

    #[test]
    fn nice_takes_an_adjustment() {
        for command in [
            "nice cargo test",
            "nice -n 5 cargo test",
            "nice -n -5 cargo test",
            "nice -n5 cargo test",
            "nice --adjustment=5 cargo test",
        ] {
            assert_eq!(
                unwrapped(command),
                (vec!["nice".into()], "cargo test".into()),
                "{command}"
            );
        }
        assert_eq!(error("nice -5 cargo"), "nice option -5");
        assert_eq!(error("nice -n x cargo"), "nice adjustment x");
    }

    #[test]
    fn nohup_and_time_take_almost_no_options() {
        assert_eq!(
            unwrapped("nohup cargo test"),
            (vec!["nohup".into()], "cargo test".into())
        );
        assert_eq!(
            unwrapped("time -p cargo test"),
            (vec!["time".into()], "cargo test".into())
        );
        assert_eq!(error("nohup -x cargo"), "nohup option -x");
        assert_eq!(error("time -l cargo"), "time option -l");
    }

    #[test]
    fn wrappers_nest() {
        let result = unwrap_words("nice -n 5 env RUST_LOG=debug timeout 60 cargo test").unwrap();

        assert_eq!(result.wrappers, vec!["nice", "env", "timeout"]);
        assert_eq!(result.env, vec![("RUST_LOG".into(), "debug".into())]);
        assert_eq!(result.name, "cargo");
    }

    #[test]
    fn a_wrapper_with_no_command_is_the_command() {
        for (command, wrappers, name) in [
            ("timeout", vec![], "timeout"),
            ("timeout 60", vec![], "timeout"),
            ("env A=1", vec![], "env"),
            ("nice -n 5", vec![], "nice"),
            ("nohup", vec![], "nohup"),
            ("env timeout", vec!["env"], "timeout"),
        ] {
            let result = unwrap_words(command).unwrap();
            assert_eq!(
                (result.wrappers, result.name.as_str(), result.env.len()),
                (
                    wrappers
                        .iter()
                        .map(|w: &&str| w.to_string())
                        .collect::<Vec<_>>(),
                    name,
                    0
                ),
                "{command}"
            );
        }
    }
}
