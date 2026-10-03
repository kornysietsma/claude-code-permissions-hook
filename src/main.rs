use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use env_logger::Env;
use log::info;
use std::path::PathBuf;
use std::process::ExitCode;

use tool_gate_hook::auditing::{Decision, audit_tool_use};
use tool_gate_hook::hook_io::HookInput;
use tool_gate_hook::{Agent, Config};

#[derive(Debug, Parser)]
#[clap(
    author,
    version,
    about = "PreToolUse hook that gates agent tool use with allow/deny/ask rules"
)]
struct Opts {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run the hook (reads JSON from stdin, outputs decision to stdout)
    Run(Target),
    /// Validate a configuration file
    Validate(Target),
}

#[derive(Debug, clap::Args)]
struct Target {
    #[clap(short, long, value_enum)]
    agent: Agent,
    /// Defaults to ~/.config/tool-gate-hook/<agent>.toml
    #[clap(short, long)]
    config: Option<PathBuf>,
}

impl Target {
    fn config_path(&self) -> Result<PathBuf> {
        match &self.config {
            Some(path) => Ok(path.clone()),
            None => {
                let home = std::env::home_dir().context("Cannot determine home directory")?;
                Ok(self.agent.default_config_path(&home))
            }
        }
    }
}

fn run_hook(target: &Target) -> Result<()> {
    let config = Config::load(&target.config_path()?)?;
    let input = HookInput::read_from_stdin().context("Failed to read hook input")?;

    // Rule evaluation arrives in plan step 2.2; until then every call passes through
    if let Some(audit) = &config.audit {
        audit_tool_use(
            &audit.file,
            audit.level,
            &input,
            Decision::Passthrough,
            None,
        );
    }
    Ok(())
}

fn run_validate_config(target: &Target) -> Result<()> {
    let config = Config::load(&target.config_path()?)?;
    info!(
        "Configuration is valid: {} rules",
        config.policy.rules.len()
    );
    Ok(())
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(Env::default().default_filter_or("warn")).init();

    match Opts::parse().command {
        // A non-zero exit is a deny in Copilot, so `run` reports errors on stderr and passes through
        Commands::Run(target) => {
            if let Err(e) = run_hook(&target) {
                eprintln!("tool-gate-hook: {e:#}");
            }
            ExitCode::SUCCESS
        }
        Commands::Validate(target) => match run_validate_config(&target) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("tool-gate-hook: {e:#}");
                ExitCode::FAILURE
            }
        },
    }
}
