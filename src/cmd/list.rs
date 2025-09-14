/*!
`list.rs`

Implements the `list` subcommand for the `mcp-hack` CLI.

Supported subjects (via `Subject` enum):
  - tools      : enumerate tool names (local MCP target)
  - tool       : alias to `tools` (singular form; prints same output)
  - resources  : placeholder
  - prompts    : placeholder

Behavior:
  - If no explicit `--target` is provided, falls back to the `MCP_TARGET`
    environment variable (if present & non-empty).
  - Local target:
      * Spawns the MCP server process (stderr suppressed) using `shared::fetch_tools_local`
      * Extracts tool names & optional description
      * Outputs either human-readable table or JSON
  - Remote target:
      * Placeholder output noting remote enumeration is not yet implemented
  - Missing target:
      * Prints a zero-count placeholder

JSON Output Shape (tools):
{
  "status": "ok",
  "subject": "tools",
  "target": "<target or null>",
  "elapsed_ms": 12,
  "count": 3,
  "tools": [
    { "name": "foo", "description": "..." },
    { "name": "bar", "description": "" }
  ]
}

Future Enhancements (not yet implemented):
  - Remote transport enumeration (HTTP/SSE/WS)
  - Filtering (--filter, --contains)
  - Pagination / ordering
  - Rich formatting (colors / widths) behind a --no-json default
  - Caching of spawned local processes

*/

use anyhow::{Context, Result};
use clap::Args;

use crate::cmd::format::{Role, StyleOptions, TableOpts, box_header, color, emoji, table};
use crate::cmd::shared::fetch_tools_local;
use crate::cmd::subject::Subject;
use crate::mcp;

/// CLI arguments for `mcp-hack list <subject>`
#[derive(Args, Debug)]
pub struct ListArgs {
    /// Subject to list (tools|tool|resources|prompts)
    pub subject: Subject,

    /// Output JSON instead of human-readable text
    #[arg(long)]
    pub json: bool,

    /// Target MCP endpoint (local command or remote URL)
    /// (Falls back to MCP_TARGET env var if omitted)
    #[arg(short = 't', long)]
    pub target: Option<String>,
}

/// Entry point for the list subcommand.
pub fn execute_list(mut args: ListArgs) -> Result<()> {
    // If user didn't supply --target, fall back to MCP_TARGET env.
    if args.target.is_none()
        && let Ok(env_t) = std::env::var("MCP_TARGET")
        && !env_t.trim().is_empty()
    {
        args.target = Some(env_t);
    }

    match args.subject {
        Subject::Tools | Subject::Tool => list_tools(args),
        Subject::Resources => list_placeholder("resources", args.json),
        Subject::Prompts => list_placeholder("prompts", args.json),
    }
}

/// List tools (plural). Subject `tool` (singular) aliases to this command to
/// avoid special-casing the output format for a single item selection here.
fn list_tools(args: ListArgs) -> Result<()> {
    let target_opt = args.target.as_deref();

    let Some(target) = target_opt else {
        if args.json {
            println!(
                "{}",
                serde_json::json!({
                    "status":"ok",
                    "subject":"tools",
                    "target": null,
                    "count":0,
                    "tools":[],
                    "note":"no target specified; use --target or MCP_TARGET"
                })
            );
        } else {
            println!("No target specified (use --target or set MCP_TARGET).");
            println!("Tools (0)");
        }
        return Ok(());
    };

    let spec =
        mcp::parse_target(target).with_context(|| format!("Failed to parse target: '{target}'"))?;

    if !spec.is_local() {
        // Remote placeholder
        if args.json {
            println!(
                "{}",
                serde_json::json!({
                    "status":"ok",
                    "subject":"tools",
                    "target": target,
                    "count":0,
                    "tools":[],
                    "note":"remote tool enumeration not implemented yet"
                })
            );
        } else {
            println!("Tools (0) - target: {target} (remote enumeration not implemented)");
        }
        return Ok(());
    }

    let tool_list = fetch_tools_local(&spec)?;
    let count = tool_list.count();

    if args.json {
        let mut items = Vec::with_capacity(count);
        for t in &tool_list.tools {
            let name = t
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("<unnamed>");
            let desc = t
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            items.push(serde_json::json!({
                "name": name,
                "description": desc
            }));
        }

        println!(
            "{}",
            serde_json::json!({
                "status":"ok",
                "subject":"tools",
                "target": target,
                "elapsed_ms": tool_list.elapsed_ms,
                "count": count,
                "tools": items
            })
        );
        return Ok(());
    }

    // Human-readable output
    // Fancy header + table formatting
    let style = StyleOptions::detect();

    let header = box_header(
        format!("{} Tools ({count})", emoji("list", &style)),
        Some(format!("target={target} • {} ms", tool_list.elapsed_ms)),
        &style,
    );
    println!("{header}");

    if count == 0 {
        println!(
            "{}",
            color(
                Role::Dim,
                format!("{} (none)", emoji("info", &style)),
                &style
            )
        );
        return Ok(());
    }

    // Build rows with columns: ["#", "NAME", "PARAMS", "DESCRIPTION"]
    // PARAMS: summarized as "p1:type, p2:type" (truncated)
    let mut table_rows: Vec<Vec<String>> = Vec::with_capacity(count);
    for (idx, t) in tool_list.tools.iter().enumerate() {
        let name = t
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed>")
            .to_string();
        let desc_raw = t
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .replace('\n', " ");

        // Parameter summary
        let mut param_pairs: Vec<String> = Vec::new();
        if let Some(schema) = t.get("input_schema").and_then(|v| v.as_object())
            && let Some(props) = schema.get("properties").and_then(|v| v.as_object())
        {
            for (pname, pobj) in props.iter().take(8) {
                let ptype = pobj
                    .as_object()
                    .and_then(|m| m.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("any");
                param_pairs.push(format!("{pname}:{ptype}"));
            }
            if props.len() > 8 {
                param_pairs.push("…".into());
            }
        }
        let param_summary = if param_pairs.is_empty() {
            "-".to_string()
        } else {
            param_pairs.join(", ")
        };

        // Truncate description for table view
        let desc = if desc_raw.len() > 90 {
            let mut s = desc_raw[..87].to_string();
            s.push_str("...");
            s
        } else {
            desc_raw
        };

        table_rows.push(vec![(idx + 1).to_string(), name, param_summary, desc]);
    }

    let tbl = table(
        &["#", "NAME", "PARAMS", "DESCRIPTION"],
        &table_rows,
        TableOpts {
            max_width: style.term_width,
            truncate: true,
            header_sep: true,
            zebra: false,
            min_col_width: 2,
        },
        &style,
    );
    println!("{tbl}");

    println!(
        "\n{} {}",
        emoji("info", &style),
        color(
            Role::Dim,
            "Use `mcp-hack get tool <name>` for detailed info on a single tool",
            &style
        )
    );

    Ok(())
}

/// Placeholder listing for unimplemented subjects.
fn list_placeholder(subject: &str, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "status":"ok",
                "subject": subject,
                "count":0,
                "items":[],
                "note":"listing for this subject not implemented yet"
            })
        );
    } else {
        println!("{subject}: listing not implemented (0 items)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::subject::Subject;
    use clap::Parser;

    // Ad-hoc parser just for testing ListArgs in isolation.
    #[derive(Parser, Debug)]
    struct TestCli {
        #[command(subcommand)]
        cmd: TestSub,
    }

    #[derive(clap::Subcommand, Debug)]
    enum TestSub {
        List(ListArgs),
    }

    #[test]
    fn clap_parses_list_tools() {
        let cli = TestCli::try_parse_from(["t", "list", "tools"]).unwrap();
        match cli.cmd {
            TestSub::List(a) => {
                assert!(matches!(a.subject, Subject::Tools));
            }
        }
    }
}
