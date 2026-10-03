use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use env_logger::Env;
use log::info;
use std::path::PathBuf;
use std::process::ExitCode;

use tool_gate_hook::auditing::audit_tool_use;
use tool_gate_hook::{
    Agent, Decision, HookInput, HookOutput, load_config, process_hook_input_with_config,
    validate_config,
};

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
    let config_path = target.config_path()?;
    let (config, deny_rules, allow_rules) = load_config(&config_path)
        .with_context(|| format!("Failed to load configuration {}", config_path.display()))?;

    let input = HookInput::read_from_stdin().context("Failed to read hook input")?;

    let result = process_hook_input_with_config(&config, &input)?;

    // Audit the decision
    audit_tool_use(
        &config.audit.audit_file,
        config.audit.audit_level,
        &input,
        result.decision,
        result.reason.as_deref(),
    );

    // Output decision to stdout (passthrough = no output)
    match result.decision {
        Decision::Allow => {
            let output = HookOutput::allow(result.reason.unwrap_or_default());
            output.write_to_stdout()?;
        }
        Decision::Deny => {
            let output = HookOutput::deny(result.reason.unwrap_or_default());
            output.write_to_stdout()?;
        }
        Decision::Passthrough => {
            // No output for passthrough
        }
    }

    // Suppress unused variable warning - rules are used for config validation
    let _ = (deny_rules, allow_rules);

    Ok(())
}

fn run_validate_config(target: &Target) -> Result<()> {
    let config_path = target.config_path()?;
    let (deny_count, allow_count) = validate_config(&config_path)
        .with_context(|| format!("Invalid configuration {}", config_path.display()))?;

    let config = tool_gate_hook::Config::load_from_file(&config_path)?;

    info!("Configuration is valid!");
    info!("  Deny rules: {}", deny_count);
    info!("  Allow rules: {}", allow_count);
    info!("  Audit file: {}", config.audit.audit_file.display());
    info!("  Audit level: {:?}", config.audit.audit_level);

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
