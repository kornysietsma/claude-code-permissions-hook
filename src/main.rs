use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tool_gate_hook::{Agent, Config, Context, auditing, validate};

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "PreToolUse hook that gates agent tool use with allow/deny/ask rules"
)]
struct Opts {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Run the hook (reads JSON from stdin, outputs decision to stdout)
    Run(Target),
    /// Validate a configuration file
    Validate(Target),
    /// Show how a shell command or payload would be judged, as the audit record it would get
    Explain(Explain),
}

#[derive(Debug, clap::Args)]
#[command(group(clap::ArgGroup::new("input").required(true).args(["command", "payload"])))]
struct Explain {
    #[command(flatten)]
    target: Target,
    /// Working directory for COMMAND; defaults to the current directory
    #[arg(long, conflicts_with = "payload")]
    cwd: Option<PathBuf>,
    /// A shell command, run through the agent's shell tool
    command: Option<String>,
    /// A JSON payload, or an audit record, to explain instead (`-` reads stdin)
    #[arg(long)]
    payload: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
struct Target {
    #[arg(short, long, value_enum)]
    agent: Agent,
    /// Defaults to ~/.config/tool-gate-hook/<agent>.toml
    #[arg(short, long)]
    config: Option<PathBuf>,
}

impl Target {
    fn config_path(&self, home: &Path) -> PathBuf {
        self.config
            .clone()
            .unwrap_or_else(|| self.agent.default_config_path(home))
    }
}

fn home() -> Result<PathBuf> {
    std::env::home_dir().context("cannot determine home directory")
}

fn read_stdin() -> Result<String> {
    let mut stdin = String::new();
    io::stdin()
        .read_to_string(&mut stdin)
        .context("cannot read stdin")?;
    Ok(stdin)
}

fn run_hook(target: &Target) -> Result<()> {
    let stdin = read_stdin()?;
    let context = Context::new(home()?);
    let config_path = target.config_path(&context.home);
    let outcome = tool_gate_hook::run(target.agent, &config_path, &stdin, &context);
    for warning in &outcome.warnings {
        eprintln!("{warning}");
    }
    if let Some((file, record)) = &outcome.audit
        && let Err(e) = auditing::append(file, record)
    {
        eprintln!("tool-gate-hook: cannot write audit record: {e:#}");
    }
    if let Some(output) = outcome.output {
        println!("{output}");
    }
    Ok(())
}

fn run_validate_config(target: &Target) -> Result<()> {
    let path = target.config_path(&home()?);
    let config = Config::load(&path, target.agent)
        .with_context(|| format!("invalid config {}", path.display()))?;
    let validation = validate::validate(target.agent, &config);
    for warning in &validation.warnings {
        eprintln!("warning: {warning}");
    }
    println!("{} is valid\n{}", path.display(), validation.summary);
    Ok(())
}

fn run_explain(explain: &Explain) -> Result<()> {
    let context = Context::new(home()?);
    let input = match (&explain.command, &explain.payload) {
        (_, Some(path)) if path.as_os_str() == "-" => read_stdin()?,
        (_, Some(path)) => {
            fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?
        }
        (Some(command), None) => {
            let cwd = match &explain.cwd {
                Some(cwd) => cwd.clone(),
                None => std::env::current_dir().context("cannot determine current directory")?,
            };
            explain
                .target
                .agent
                .shell_payload(command, &cwd)
                .to_string()
        }
        (None, None) => bail!("give a command or --payload"),
    };
    let config_path = explain.target.config_path(&context.home);
    let explanation =
        tool_gate_hook::explain(explain.target.agent, &config_path, &input, &context)?;
    for warning in &explanation.warnings {
        eprintln!("{warning}");
    }
    println!("{}", serde_json::to_string_pretty(&explanation.record)?);
    Ok(())
}

fn main() -> ExitCode {
    match Opts::parse().command {
        // A non-zero exit is a deny in Copilot, so `run` reports errors on stderr and passes through
        Commands::Run(target) => {
            if let Err(e) = run_hook(&target) {
                eprintln!("tool-gate-hook: {e:#}");
            }
            ExitCode::SUCCESS
        }
        Commands::Validate(target) => exit_code(run_validate_config(&target)),
        Commands::Explain(explain) => exit_code(run_explain(&explain)),
    }
}

fn exit_code(result: Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tool-gate-hook: {e:#}");
            ExitCode::FAILURE
        }
    }
}
