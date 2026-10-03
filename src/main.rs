use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use env_logger::Env;
use log::info;
use std::path::PathBuf;

use tool_gate_hook::auditing::audit_tool_use;
use tool_gate_hook::{
    Decision, HookInput, HookOutput, load_config, process_hook_input_with_config, validate_config,
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
    Run {
        #[clap(short, long, value_parser)]
        config: PathBuf,
    },
    /// Validate a configuration file
    Validate {
        #[clap(short, long, value_parser)]
        config: PathBuf,
    },
}

fn run_hook(config_path: PathBuf) -> Result<()> {
    let (config, deny_rules, allow_rules) =
        load_config(&config_path).context("Failed to load configuration")?;

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

fn run_validate_config(config_path: PathBuf) -> Result<()> {
    let (deny_count, allow_count) = validate_config(&config_path)?;

    let config = tool_gate_hook::Config::load_from_file(&config_path)?;

    info!("Configuration is valid!");
    info!("  Deny rules: {}", deny_count);
    info!("  Allow rules: {}", allow_count);
    info!("  Audit file: {}", config.audit.audit_file.display());
    info!("  Audit level: {:?}", config.audit.audit_level);

    Ok(())
}

fn main() -> Result<()> {
    // Initialize diagnostic logger from RUST_LOG env var (default: warn)
    env_logger::Builder::from_env(Env::default().default_filter_or("warn")).init();

    let opts = Opts::parse();

    match opts.command {
        Commands::Run { config } => run_hook(config),
        Commands::Validate { config } => run_validate_config(config),
    }
}
