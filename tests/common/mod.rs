use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use tool_gate_hook::{Agent, Context, Outcome, run};

pub fn fixture(agent: &str, name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("tests/fixtures/{agent}/{name}.json"));
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub fn no_home() -> Context {
    Context::new(PathBuf::from("/nonexistent-home"))
}

/// Runs the hook with `config` written to `<agent>.toml` in a temp directory
pub fn run_with_config(agent: Agent, config: &str, stdin: &str) -> Outcome {
    let dir = TempDir::new().unwrap();
    let config_path = dir.path().join(format!("{}.toml", agent.name()));
    fs::write(&config_path, config).unwrap();
    run(agent, &config_path, stdin, &no_home())
}
