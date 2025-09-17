use anyhow::Result;
use clap::{Parser, Subcommand};

mod cmd;
mod mcp;
mod utils;

use cmd::{
    ExecArgs, FuzzArgs, GetArgs, ListArgs, execute_exec, execute_fuzz, execute_get, execute_list,
};

/// MCP Hack CLI
///
/// Implemented subjects: `tools`, `tool` (plural vs single); `resources` / `prompts` are placeholders.
///
/// Examples:
///   mcp-hack list tools -t "npx -y @modelcontextprotocol/server-everything"
///   mcp-hack get tool scan_with_dalfox -t "dalfox server --type=mcp" --json
///   mcp-hack get tool -t "dalfox server --type=mcp"            (interactive choose)
///   mcp-hack exec tool scan_with_dalfox -t "dalfox server --type=mcp" --param url=https://target --json
///
/// Targets:
///   - Local command (spawned child process)  [supported]
///   - Remote URL (http/https/ws/wss)         [parsing only; remote ops not yet implemented]
///
/// Global flags / env:
///   -v / -vv increase verbosity; -q quiet
///   -t / --target or MCP_TARGET env for default target
///   -H / --header KEY=VALUE (reserved for future remote support)
///
/// Output:
///   Human-readable tables / boxes or --json`.
#[derive(Parser, Debug)]
#[command(
    name = "mcp-hack",
    version,
    author,
    about = "MCP Hack - experimental security / exploration CLI for MCP",
    propagate_version = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Increase verbosity (-v, -vv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Silence all non-error output
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Default target (local command or remote URL)
    #[arg(short = 't', long = "target", global = true, value_name = "TARGET")]
    target: Option<String>,

    /// Extra header(s) for remote transports (repeatable KEY=VALUE)
    #[arg(short = 'H', long = "header", global = true, value_name = "KEY=VALUE")]
    headers: Vec<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// List subject item names
    List(ListArgs),

    /// Get detailed subject items
    Get(GetArgs),

    /// Execute (invoke) a tool
    Exec(ExecArgs),

    /// Fuzz a tool with a wordlist
    Fuzz(FuzzArgs),
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let level = utils::derive_level(cli.verbose, cli.quiet);
    utils::init_logging(level);

    // Effective global target (CLI flag > MCP_TARGET env)
    let global_target = cli.target.clone().or_else(|| {
        std::env::var("MCP_TARGET")
            .ok()
            .filter(|s| !s.trim().is_empty())
    });

    // Validate target syntax early if provided
    if let Some(t) = &global_target
        && let Err(e) = mcp::parse_target(t) {
            eprintln!("Invalid target '{}': {}", t, e);
            std::process::exit(2);
        }

    match cli.command {
        Commands::List(mut args) => {
            if args.target.is_none() {
                args.target = global_target.clone();
            }
            execute_list(args)
        }
        Commands::Get(mut args) => {
            if args.target.is_none() {
                args.target = global_target.clone();
            }
            execute_get(args)
        }
        Commands::Exec(mut args) => {
            if args.target.is_none() {
                args.target = global_target.clone();
            }
            execute_exec(args)
        }
        Commands::Fuzz(mut args) => {
            if args.target.is_none() {
                args.target = global_target.clone();
            }
            execute_fuzz(args)
        }
    }
}
