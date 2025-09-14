use anyhow::Result;
use clap::{Parser, Subcommand};

mod cmd;
mod mcp;
mod utils;

use cmd::{ExecArgs, GetArgs, ListArgs};

/// MCP Hack - Refactored CLI (modularized: see cmd/{list,get,exec,subject,shared}.rs)
///
/// New command layout (modular):
///   mcp-hack list <tools|resources|prompts> [--json] [-t "<target>"]
///   mcp-hack get  <tools|tool|resources|prompts> [NAME] [--json] [-t "<target>"]
///   mcp-hack exec tools <tool-name> [--param k=v ...] [-t "<target>"] [--json] [--raw]
///
/// Notes:
///   - get tools : detailed info for all tools
///   - get tool  : detailed info for a single tool; if NAME omitted, interactive selection prompts
///
/// Global flags / env:
///   -v / -vv        Increase verbosity
///   -q / --quiet    Errors only
///   -t / --target   Default target (or MCP_TARGET env); -H/--header KEY=VALUE (repeatable)
///   MCP_TARGET      Environment fallback if -t not provided
///
/// Subjects:
///   tools      - Implemented (enumerates / invokes)
///   tool       - Single tool detail (interactive if no name)
///   resources  - Placeholder
///   prompts    - Placeholder
///
/// Target kinds:
///   Local command (spawned): e.g.  "npx -y @modelcontextprotocol/server-everything"
///   Remote URL (http/https/ws/wss): placeholder only (no enumeration yet)
///
/// Examples:
///   mcp-hack list tools -t "npx -y @modelcontextprotocol/server-everything"
///   mcp-hack get tools -t "npx -y @modelcontextprotocol/server-everything" --json
///   mcp-hack get tool scan_with_dalfox -t "dalfox server --type=mcp" --json
///   mcp-hack get tool -t "dalfox server --type=mcp"   (interactive selection)
///   mcp-hack exec tools scan_with_dalfox -t "dalfox server --type=mcp" --param url=https://target --json
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let level = utils::derive_level(cli.verbose, cli.quiet);
    utils::init_logging(level);

    // Determine effective global target (CLI flag > MCP_TARGET env)
    let global_target = cli.target.clone().or_else(|| {
        std::env::var("MCP_TARGET")
            .ok()
            .filter(|s| !s.trim().is_empty())
    });

    // Validate if present
    if let Some(t) = &global_target
        && let Err(e) = mcp::parse_target(t)
    {
        eprintln!("Invalid target '{}': {e}", t);
        std::process::exit(2);
    }

    match cli.command {
        Commands::List(mut args) => {
            if args.target.is_none() {
                args.target = global_target.clone();
            }
            cmd::execute_list(args)
        }
        Commands::Get(mut args) => {
            if args.target.is_none() {
                args.target = global_target.clone();
            }
            cmd::execute_get(args)
        }
        Commands::Exec(mut args) => {
            if args.target.is_none() {
                args.target = global_target.clone();
            }
            cmd::execute_exec(args)
        }
    }
}
