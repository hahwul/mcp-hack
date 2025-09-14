/*!
`get.rs`

Implements the `get` subcommand for the `mcp-hack` CLI.

Supported subjects (via `Subject` enum):
  - tools (plural): return detailed information for all tools
  - tool  (singular): return detailed information for exactly one tool
                       (if no name provided, interactive selection)
  - resources / prompts: placeholders (not implemented yet)

Human Output Enhancements (fancy formatting):
  - Boxed headers with target + elapsed time
  - Parameter table with columns: NAME | TYPE | REQ | DESCRIPTION
  - Color + emoji (disabled via NO_COLOR / NO_EMOJI env vars)
  - Summary hints at bottom

Target Handling:
  - Uses `--target/-t` if supplied
  - Otherwise falls back to the `MCP_TARGET` environment variable
  - Only local (process) targets are implemented today; remote is placeholder

JSON Output Shapes (unchanged):

1) get tools
{
  "status":"ok",
  "subject":"tools",
  "target":"<...>",
  "elapsed_ms": 12,
  "count": 2,
  "tools":[
    {
      "name":"toolA",
      "description":"desc",
      "parameters":[
        {"name":"id","type":"integer","required":true,"description":""}
      ]
    }
  ]
}

2) get tool <name> (or interactively chosen)
{
  "status":"ok",
  "subject":"tool",
  "target":"<...>",
  "elapsed_ms": 7,
  "name":"toolA",
  "tool": { <raw tool object> },
  "parameters":[
    {"name":"id","type":"integer","required":true,"description":""}
  ]
}

Placeholders (resources/prompts):
{
  "status":"ok",
  "subject":"resources",
  "count":0,
  "items":[],
  "note":"get for this subject not implemented yet"
}

Future Enhancements:
  - Remote transports (HTTP/SSE/WS)
  - Filtering (--filter / --name <pattern>)
  - Rich formatting (table columns / color)
  - Schema validation & nested parameter rendering
  - Optional caching of spawned MCP server process
*/

use anyhow::{Context, Result};
use clap::Args;
use std::io::{self, Write};

use crate::cmd::format::{StyleOptions, box_header, emoji};
use crate::cmd::shared::fetch_tools_local;
use crate::cmd::subject::Subject;
use crate::mcp;

/// CLI arguments for `mcp-hack get <subject> [NAME]`
#[derive(Args, Debug)]
pub struct GetArgs {
    /// Subject (tools|tool|resources|prompts)
    pub subject: Subject,

    /// Optional tool name (used only when subject=tool). If omitted, interactive selection is offered.
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    /// Output JSON instead of human-readable text
    #[arg(long)]
    pub json: bool,

    /// Target MCP endpoint (local command or remote URL)
    /// (Falls back to MCP_TARGET env var if omitted)
    #[arg(short = 't', long)]
    pub target: Option<String>,
}

/// Entrypoint for `get` subcommand.
pub fn execute_get(mut args: GetArgs) -> Result<()> {
    // Fallback to environment target if not supplied.
    if args.target.is_none()
        && let Ok(env_t) = std::env::var("MCP_TARGET")
        && !env_t.trim().is_empty()
    {
        args.target = Some(env_t);
    }

    match args.subject {
        Subject::Tools => get_all_tools(args),
        Subject::Tool => get_single_tool(args),
        Subject::Resources => get_placeholder("resources", args.json),
        Subject::Prompts => get_placeholder("prompts", args.json),
    }
}

/* -------------------------------------------------------------------------- */
/* Tools (plural)                                                              */
/* -------------------------------------------------------------------------- */

fn get_all_tools(args: GetArgs) -> Result<()> {
    let Some(target) = args.target.as_deref() else {
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
            println!("Tools: (none)");
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
                    "note":"remote tool retrieval not implemented yet"
                })
            );
        } else {
            println!("(remote) Detailed tool retrieval not implemented for {target}");
        }
        return Ok(());
    }

    let tool_list = fetch_tools_local(&spec)?;
    if args.json {
        // Build enriched JSON objects with parameters
        let mut enriched = Vec::with_capacity(tool_list.count());
        for t in &tool_list.tools {
            let name = t
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("<unnamed>")
                .to_string();
            let desc = t
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let params = extract_params(t);
            enriched.push(serde_json::json!({
                "name": name,
                "description": desc,
                "parameters": params.into_iter().map(|(n,t,r,d)| serde_json::json!({
                    "name":n,"type":t,"required":r,"description":d
                })).collect::<Vec<_>>()
            }));
        }

        println!(
            "{}",
            serde_json::json!({
                "status":"ok",
                "subject":"tools",
                "target": target,
                "elapsed_ms": tool_list.elapsed_ms,
                "count": tool_list.count(),
                "tools": enriched
            })
        );
        return Ok(());
    }

    // Human output
    let style = StyleOptions::detect();
    let header = box_header(
        format!(
            "{} Tools Detail ({})",
            emoji("list", &style),
            tool_list.count()
        ),
        Some(format!("target={target} • {} ms", tool_list.elapsed_ms)),
        &style,
    );
    println!("{header}");
    if tool_list.tools.is_empty() {
        println!("(none)");
        return Ok(());
    }
    for (idx, t) in tool_list.tools.iter().enumerate() {
        let name = t
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed>");
        let desc = t
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("<no description>");
        println!();
        println!("#{}: {}", idx + 1, name);
        println!(
            "  Description: {}",
            if desc.is_empty() { "<none>" } else { desc }
        );
        let params = extract_params(t);
        if params.is_empty() {
            println!("  Parameters: (none)");
        } else {
            // Fancy parameter table
            use crate::cmd::format::{StyleOptions, TableOpts, table};
            let style = StyleOptions::detect();
            let mut rows_vec: Vec<Vec<String>> = Vec::new();
            for (pn, pt, req, pd) in params {
                rows_vec.push(vec![
                    pn,
                    pt,
                    if req { "yes".into() } else { "no".into() },
                    if pd.is_empty() { "-".into() } else { pd },
                ]);
            }
            let tbl = table(
                &["NAME", "TYPE", "REQ", "DESCRIPTION"],
                &rows_vec,
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
        }
    }

    Ok(())
}

/* -------------------------------------------------------------------------- */
/* Singular tool                                                              */
/* -------------------------------------------------------------------------- */

fn get_single_tool(args: GetArgs) -> Result<()> {
    let Some(target) = args.target.as_deref() else {
        if args.json {
            println!(
                "{}",
                serde_json::json!({
                    "status":"ok",
                    "subject":"tool",
                    "target": null,
                    "tool": null,
                    "note":"no target specified; use --target or MCP_TARGET"
                })
            );
        } else {
            println!("No target specified (use --target or MCP_TARGET).");
        }
        return Ok(());
    };

    let spec =
        mcp::parse_target(target).with_context(|| format!("Failed to parse target: '{target}'"))?;

    if !spec.is_local() {
        if args.json {
            println!(
                "{}",
                serde_json::json!({
                    "status":"ok",
                    "subject":"tool",
                    "target": target,
                    "tool": null,
                    "note":"remote single-tool retrieval not implemented yet"
                })
            );
        } else {
            println!("(remote) Single tool retrieval not implemented for {target}");
        }
        return Ok(());
    }

    let tool_list = fetch_tools_local(&spec)?;
    if tool_list.tools.is_empty() {
        if args.json {
            println!(
                "{}",
                serde_json::json!({
                    "status":"ok",
                    "subject":"tool",
                    "target": target,
                    "tool": null,
                    "note":"no tools"
                })
            );
        } else {
            println!("No tools available.");
        }
        return Ok(());
    }

    // Determine final tool name (either from args.name or interactive selection)
    let final_name = if let Some(n) = args.name {
        n
    } else {
        interactive_select_tool(&tool_list.tools)?
    };

    // Locate tool
    let mut found: Option<serde_json::Value> = None;
    for t in &tool_list.tools {
        if let Some(n) = t.get("name").and_then(|v| v.as_str())
            && n.eq_ignore_ascii_case(&final_name)
        {
            found = Some(t.clone());
            break;
        }
    }

    let Some(tool_obj) = found else {
        if args.json {
            println!(
                "{}",
                serde_json::json!({
                    "status":"error",
                    "error":"tool not found",
                    "requested": final_name,
                    "subject":"tool",
                    "target": target
                })
            );
        } else {
            println!("Tool '{}' not found.", final_name);
        }
        return Ok(());
    };

    let params = extract_params(&tool_obj);

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "status":"ok",
                "subject":"tool",
                "target": target,
                "elapsed_ms": tool_list.elapsed_ms,
                "name": final_name,
                "tool": tool_obj,
                "parameters": params.iter().map(|(n,t,r,d)| serde_json::json!({
                    "name":n,"type":t,"required":r,"description":d
                })).collect::<Vec<_>>()
            })
        );
        return Ok(());
    }

    // Human output
    let style = StyleOptions::detect();
    let header = box_header(
        format!("{} Tool: {}", emoji("tool", &style), final_name),
        Some(format!("target={target} • {} ms", tool_list.elapsed_ms)),
        &style,
    );
    println!("{header}");
    if let Some(desc) = tool_obj.get("description").and_then(|v| v.as_str()) {
        println!(
            "Description: {}",
            if desc.is_empty() { "<none>" } else { desc }
        );
    } else {
        println!("Description: <none>");
    }
    if params.is_empty() {
        println!("Parameters: (none)");
    } else {
        use crate::cmd::format::{StyleOptions, TableOpts, table};
        let style = StyleOptions::detect();
        let mut rows: Vec<Vec<String>> = Vec::new();
        for (n, t, r, d) in params {
            rows.push(vec![
                n,
                t,
                if r { "yes".into() } else { "no".into() },
                if d.is_empty() { "-".into() } else { d },
            ]);
        }
        let tbl = table(
            &["NAME", "TYPE", "REQ", "DESCRIPTION"],
            &rows,
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
    }

    Ok(())
}

/* -------------------------------------------------------------------------- */
/* Placeholder subjects                                                        */
/* -------------------------------------------------------------------------- */

fn get_placeholder(subject: &str, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "status":"ok",
                "subject": subject,
                "count":0,
                "items":[],
                "note":"get for this subject not implemented yet"
            })
        );
    } else {
        println!("{subject}: detailed retrieval not implemented (0 items)");
    }
    Ok(())
}

/* -------------------------------------------------------------------------- */
/* Helpers                                                                     */
/* -------------------------------------------------------------------------- */

/// Extract parameter list from a raw tool JSON object.
///
/// Return vector of (name, type, required, description)
fn extract_params(tool_obj: &serde_json::Value) -> Vec<(String, String, bool, String)> {
    let mut params = Vec::new();
    let Some(schema) = tool_obj.get("input_schema").and_then(|v| v.as_object()) else {
        return params;
    };

    // Collect required set
    let required: std::collections::HashSet<String> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    if let Some(props) = schema.get("properties").and_then(|v| v.as_object()) {
        for (pname, pobj) in props {
            let (ptype, pdesc) = if let Some(obj) = pobj.as_object() {
                (
                    obj.get("type").and_then(|v| v.as_str()).unwrap_or("any"),
                    obj.get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                )
            } else {
                ("unknown", "")
            };
            let is_required = required.contains(pname);
            params.push((
                pname.clone(),
                ptype.to_string(),
                is_required,
                pdesc.to_string(),
            ));
        }
    }

    params
}

/// Interactive selection for a single tool (used when `get tool` has no name).
fn interactive_select_tool(tools: &[serde_json::Value]) -> Result<String> {
    println!("Select a tool:");
    for (i, t) in tools.iter().enumerate() {
        let nm = t
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed>");
        println!("  [{}] {}", i + 1, nm);
    }
    print!("Enter number (1-{}): ", tools.len());
    let _ = io::stdout().flush();
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let trimmed = line.trim();
    // Try numeric selection
    if let Ok(idx) = trimmed.parse::<usize>()
        && idx >= 1
        && idx <= tools.len()
    {
        let nm = tools[idx - 1]
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("<unnamed>");
        return Ok(nm.to_string());
    }
    // Fallback: treat trimmed input as direct name
    if trimmed.is_empty() {
        anyhow::bail!("invalid selection");
    }
    Ok(trimmed.to_string())
}

/* -------------------------------------------------------------------------- */
/* Tests (basic)                                                               */
/* -------------------------------------------------------------------------- */
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::subject::Subject;

    #[test]
    fn extract_params_empty() {
        let val = serde_json::json!({"name":"x"});
        let p = extract_params(&val);
        assert!(p.is_empty());
    }

    #[test]
    fn extract_params_basic() {
        let val = serde_json::json!({
            "name":"demo",
            "input_schema":{
                "type":"object",
                "required":["a"],
                "properties":{
                    "a":{"type":"integer","description":"id"},
                    "b":{"type":"boolean"}
                }
            }
        });
        let mut p = extract_params(&val);
        p.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].0, "a");
        assert_eq!(p[0].1, "integer");
        assert!(p[0].2);
        assert_eq!(p[0].3, "id");
        assert_eq!(p[1].0, "b");
        assert_eq!(p[1].1, "boolean");
        assert!(!p[1].2);
    }

    #[test]
    fn interactive_select_tool_fallback_name() {
        // We cannot simulate stdin easily here; just test helper functions above.
        let _ = Subject::Tools; // silence unused import in this context
    }
}
