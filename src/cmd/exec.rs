/*!
`exec.rs`

Implements the `exec` subcommand for the `mcp-hack` CLI, allowing direct invocation
of a single MCP tool exposed by a local MCP server process.

Current Capabilities:
  - Local process targets (spawned via child process transport)
  - Subject filter: prefers `tool` (singular); `tools` accepted with deprecation warning
  - Tool selection by explicit name argument
  - Parameter injection via:
      --param KEY=VALUE              (repeatable)
      --param-file params.(json|yaml) (merged; CLI --param overrides file entries)
      --interactive                  (prompt for missing required params)
  - Basic type coercion (integer / number / boolean / array) using shared helpers
  - JSON or human-readable output
  - Raw result inclusion with --raw

Not Yet Implemented:
  - Remote targets (HTTP/SSE/WS)
  - Tool discovery caching / persistent process reuse
  - Complex schema validation (nested objects, enums, etc.)
  - Concurrency / multiple invocations
  - Timeout / cancellation knobs

JSON Success Output (summary mode):
{
  "status": "ok",
  "subject": "tool",
  "tool": "example_tool",
  "target": "...",
  "elapsed_ms": 42,
  "arguments": { ... },
  "result_summary": { ...serialized call result... }
}

JSON Success Output (--raw):
{
  "status": "ok",
  "subject": "tool",
  "tool": "example_tool",
  "target": "...",
  "elapsed_ms": 42,
  "arguments": { ... },
  "result": { ...full raw call result object... }
}

JSON Error Output:
{
  "status":"error",
  "error":"message"
}

*/

use anyhow::{Context, Result};
use clap::Args;
use std::io::{self, Write};
use std::time::Instant;

use super::subject::Subject;
use crate::cmd::format::{Role, StyleOptions, TableOpts, box_header, color, emoji, table};
use crate::cmd::shared::{
    build_arguments_from_schema, find_tool_case_insensitive, summarize_call_result,
};
use crate::mcp;

/* -------------------------------------------------------------------------- */
/* Argument Struct                                                            */
/* -------------------------------------------------------------------------- */

#[derive(Args, Debug)]
pub struct ExecArgs {
    /// Subject to execute ('tool' preferred; 'tools' is a deprecated alias)
    pub subject: Subject,

    /// Tool name to invoke
    #[arg(value_name = "TOOL")]
    pub tool: String,

    /// Provide parameter (KEY=VALUE), repeatable
    #[arg(long = "param", value_name = "KEY=VALUE")]
    pub params: Vec<String>,

    /// Load parameters from file (JSON or YAML). CLI --param overrides file entries
    #[arg(long = "param-file", value_name = "PATH")]
    pub param_file: Option<String>,

    /// Prompt interactively for missing required parameters
    #[arg(long)]
    pub interactive: bool,

    /// Target MCP endpoint (local command or remote URL). Falls back to MCP_TARGET env.
    #[arg(short = 't', long)]
    pub target: Option<String>,

    /// Output JSON
    #[arg(long)]
    pub json: bool,

    /// Include raw MCP call result (instead of summary) in JSON / human output
    #[arg(long)]
    pub raw: bool,
}

/* -------------------------------------------------------------------------- */
/* Public Entry Point                                                         */
/* -------------------------------------------------------------------------- */

pub fn execute_exec(mut args: ExecArgs) -> Result<()> {
    // Subject check & deprecation handling
    if matches!(args.subject, Subject::Tools) {
        // Backward compatibility: allow plural with a warning
        if args.json {
            eprintln!(r#"{{"warning":"subject 'tools' is deprecated; use 'tool'"}}"#);
        } else {
            let style = StyleOptions::detect();
            println!(
                "{} {}",
                emoji("info", &style),
                color(
                    Role::Dim,
                    "Subject 'tools' is deprecated; use 'tool'",
                    &style
                )
            );
        }
    } else if !matches!(args.subject, Subject::Tool) {
        return output_error(args.json, "exec currently supports only subject 'tool'");
    }

    // Tool name validation
    let tool_name_owned = args.tool.trim().to_string();
    if tool_name_owned.is_empty() {
        return output_error(args.json, "tool name cannot be empty");
    }

    // Determine target (CLI > env)
    if args.target.is_none()
        && let Ok(env_t) = std::env::var("MCP_TARGET")
        && !env_t.trim().is_empty()
    {
        args.target = Some(env_t);
    }
    let target_raw = match &args.target {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            return output_error(
                args.json,
                "no target specified (use --target or MCP_TARGET)",
            );
        }
    };

    // Parse target spec
    let spec = mcp::parse_target(&target_raw)
        .with_context(|| format!("Failed to parse target: '{target_raw}'"))?;

    if !spec.is_local() {
        return output_error(args.json, "remote exec not implemented yet");
    }

    // Collect parameters from CLI
    let mut provided: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for kv in &args.params {
        if let Some((k, v)) = kv.split_once('=') {
            let key = k.trim();
            if key.is_empty() {
                return output_error(args.json, &format!("invalid --param (empty key): {kv}"));
            }
            provided.insert(key.to_string(), v.trim().to_string());
        } else {
            return output_error(
                args.json,
                &format!("invalid --param (expected KEY=VALUE): {kv}"),
            );
        }
    }

    // Load param file if specified (merge non-conflicting keys)
    if let Some(ref pf) = args.param_file
        && let Err(e) = load_param_file_into_map(pf, &mut provided)
    {
        return output_error(args.json, &e.to_string());
    }

    // Build runtime + spawn + list tools + interactive prompts + call tool
    let started = Instant::now();
    let result = invoke_tool(
        &spec,
        &tool_name_owned,
        provided,
        args.interactive,
        args.json,
    );

    let elapsed_ms = started.elapsed().as_millis();

    match result {
        Ok((final_args_map, call_result)) => {
            if args.json {
                // JSON output
                let mut base = serde_json::json!({
                    "status":"ok",
                    "subject": "tool",
                    "tool": tool_name_owned,
                    "target": target_raw,
                    "elapsed_ms": elapsed_ms,
                    "arguments": final_args_map,
                });
                if args.raw {
                    if let serde_json::Value::Object(ref mut map) = base {
                        map.insert(
                            "result".to_string(),
                            serde_json::to_value(&call_result)
                                .unwrap_or_else(|_| serde_json::json!({"error":"serialize"})),
                        );
                    }
                } else if let serde_json::Value::Object(ref mut map) = base {
                    map.insert(
                        "result_summary".to_string(),
                        summarize_call_result(&call_result),
                    );
                }
                println!(
                    "{}",
                    serde_json::to_string_pretty(&base).unwrap_or_else(|_| base.to_string())
                );
            } else {
                // Fancy human-readable output
                let style = StyleOptions::detect();

                // Header box
                let header = box_header(
                    format!(
                        "{} Exec Success ({})",
                        emoji("success", &style),
                        tool_name_owned
                    ),
                    Some(format!("target={target_raw} • {elapsed_ms} ms")),
                    &style,
                );
                println!("{header}");

                // Arguments table (if any)
                if final_args_map.is_empty() {
                    println!(
                        "{}",
                        color(
                            Role::Dim,
                            format!("{} No arguments supplied", emoji("info", &style)),
                            &style
                        )
                    );
                } else {
                    let mut arg_rows: Vec<Vec<String>> = Vec::new();
                    for (k, v) in &final_args_map {
                        let v_str = match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        arg_rows.push(vec![k.clone(), v_str]);
                    }
                    // stable ordering
                    arg_rows.sort_by(|a, b| a[0].cmp(&b[0]));
                    let arg_table = table(
                        &["NAME", "VALUE"],
                        &arg_rows,
                        TableOpts {
                            max_width: style.term_width,
                            truncate: true,
                            header_sep: true,
                            zebra: false,
                            min_col_width: 2,
                        },
                        &style,
                    );
                    println!("{}", color(Role::Accent, "Arguments:", &style));
                    println!("{arg_table}");
                }

                println!();

                if args.raw {
                    println!(
                        "{} {}",
                        emoji("info", &style),
                        color(Role::Accent, "Raw Result:", &style)
                    );
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &serde_json::to_value(&call_result)
                                .unwrap_or_else(|_| serde_json::json!({"error":"serialize"}))
                        )
                        .unwrap_or_else(|_| "<serialize error>".into())
                    );
                } else {
                    println!(
                        "{} {}",
                        emoji("info", &style),
                        color(Role::Accent, "Result Summary:", &style)
                    );
                    let summary = summarize_call_result(&call_result);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&summary)
                            .unwrap_or_else(|_| summary.to_string())
                    );
                    println!(
                        "\n{} {}",
                        emoji("info", &style),
                        color(
                            Role::Dim,
                            "Use --raw to see full call result payload",
                            &style
                        )
                    );
                }
            }
        }
        Err(e) => {
            return output_error(args.json, &e.to_string());
        }
    }

    Ok(())
}

/* -------------------------------------------------------------------------- */
/* Core Invocation Logic                                                       */
/* -------------------------------------------------------------------------- */

fn invoke_tool(
    spec: &crate::mcp::TargetSpec,
    tool_name: &str,
    mut provided: std::collections::HashMap<String, String>,
    interactive: bool,
    json_mode: bool,
) -> Result<(
    serde_json::Map<String, serde_json::Value>,
    rmcp::model::CallToolResult,
)> {
    use rmcp::ServiceExt;
    use rmcp::model::CallToolRequestParam;
    use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
    use tokio::process::Command;

    // Spawn runtime (main is currently sync)
    let rt = tokio::runtime::Runtime::new().context("Failed to create Tokio runtime")?;

    rt.block_on(async {
        // Extract local program/args
        let (program, args_vec) = match spec {
            crate::mcp::TargetSpec::LocalCommand { program, args, .. } => {
                (program.clone(), args.clone())
            }
            _ => anyhow::bail!("invoke_tool only supports local process targets"),
        };

        // Spawn child MCP process
        let service = ()
            .serve(TokioChildProcess::new(Command::new(&program).configure(
                |c| {
                    for a in &args_vec {
                        c.arg(a);
                    }
                    // Silence child stderr (banners/log noise) while preserving stdout for protocol
                    c.stderr(std::process::Stdio::null());
                },
            ))?)
            .await
            .with_context(|| format!("Failed to spawn MCP process: {}", program))?;

        // Enumerate tools
        let tools_resp = service
            .list_tools(Default::default())
            .await
            .context("Failed to list tools")?;

        let tools_val = serde_json::to_value(&tools_resp).unwrap_or(serde_json::Value::Null);
        let tool_obj_val = find_tool_case_insensitive(&tools_val, tool_name)
            .ok_or_else(|| anyhow::anyhow!(format!("tool '{}' not found", tool_name)))?;

        let tool_obj = tool_obj_val
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("tool JSON is not an object"))?;

        // Interactive prompt for missing required parameters (if requested)
        if interactive {
            prompt_for_missing_required(tool_obj, &mut provided)?;
        }

        // Build argument object (schema-driven)
        let arg_obj = build_arguments_from_schema(tool_obj, &provided)
            .context("Failed to build arguments")?;

        // Invoke tool
        let call_result = service
            .call_tool(CallToolRequestParam {
                name: tool_name.to_string().into(),
                arguments: if arg_obj.is_empty() {
                    None
                } else {
                    Some(arg_obj.clone())
                },
            })
            .await
            .with_context(|| format!("tool invocation failed: {}", tool_name))?;

        // Attempt graceful shutdown
        let _ = service.cancel().await;

        if json_mode {
            // For JSON output we want to pass through the argument map unchanged
            Ok((arg_obj, call_result))
        } else {
            // In human mode we also keep the same map
            Ok((arg_obj, call_result))
        }
    })
}

/* -------------------------------------------------------------------------- */
/* Interactive Prompting                                                       */
/* -------------------------------------------------------------------------- */

fn prompt_for_missing_required(
    tool_obj: &serde_json::Map<String, serde_json::Value>,
    provided: &mut std::collections::HashMap<String, String>,
) -> Result<()> {
    // Extract schema
    let schema = tool_obj.get("input_schema").and_then(|v| v.as_object());
    let Some(schema_obj) = schema else {
        return Ok(()); // No schema -> nothing to prompt
    };

    // Collect required
    let required: std::collections::HashSet<&str> = schema_obj
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str())
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    let props = schema_obj
        .get("properties")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    for (pname, pobj) in props {
        if !required.contains(pname.as_str()) {
            continue;
        }
        if provided.contains_key(&pname) {
            continue;
        }
        // Determine type (for display)
        let ptype = pobj
            .as_object()
            .and_then(|m| m.get("type"))
            .and_then(|v| v.as_str())
            .unwrap_or("string");
        loop {
            print!(
                "Enter value for required param '{}'(type: {}): ",
                pname, ptype
            );
            let _ = io::stdout().flush();
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            let val = line.trim();
            if val.is_empty() {
                println!("  (value required)");
                continue;
            }
            // (We do not coerce here; final coercion is handled by build_arguments_from_schema / coerce_value)
            provided.insert(pname.clone(), val.to_string());
            break;
        }
    }
    Ok(())
}

/* -------------------------------------------------------------------------- */
/* Parameter File Loading                                                      */
/* -------------------------------------------------------------------------- */

fn load_param_file_into_map(
    path: &str,
    provided: &mut std::collections::HashMap<String, String>,
) -> Result<()> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read param file: {path}"))?;
    let lower = path.to_ascii_lowercase();

    let value: serde_json::Value = if lower.ends_with(".yaml") || lower.ends_with(".yml") {
        let yaml_v: serde_yaml::Value =
            serde_yaml::from_str(&raw).context("failed to parse YAML param file")?;
        serde_json::to_value(yaml_v).context("failed to convert YAML to JSON")?
    } else {
        serde_json::from_str(&raw).context("failed to parse JSON param file")?
    };

    let obj = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("param file root must be an object"))?;

    for (k, v) in obj {
        if provided.contains_key(k) {
            continue; // CLI overrides file
        }
        let s = match v {
            serde_json::Value::String(sv) => sv.clone(),
            _ => v.to_string(),
        };
        provided.insert(k.clone(), s);
    }
    Ok(())
}

/* -------------------------------------------------------------------------- */
/* Output Helpers                                                              */
/* -------------------------------------------------------------------------- */

fn output_error(json: bool, msg: &str) -> Result<()> {
    if json {
        let err = serde_json::json!({"status":"error","error":msg});
        println!(
            "{}",
            serde_json::to_string_pretty(&err).unwrap_or_else(|_| err.to_string())
        );
    } else {
        // Fancy red error box for human output
        let style = StyleOptions::detect();
        let title = format!("{} Exec Error", emoji("error", &style));
        // Color the message in red (Role::Error)
        let subtitle = color(Role::Error, msg, &style);
        let boxed = box_header(title, Some(subtitle), &style);
        println!("{boxed}");
        println!(
            "{} {}",
            emoji("info", &style),
            color(
                Role::Dim,
                "Re-run with --json for machine-readable output or add --raw for full result payload (if available).",
                &style
            )
        );
    }
    anyhow::bail!(msg.to_string())
}

/* -------------------------------------------------------------------------- */
/* Tests (basic components)                                                    */
/* -------------------------------------------------------------------------- */
#[cfg(test)]
mod tests {
    use super::*;
    // Import only for tests (runtime code does not need coerce_value directly)
    use crate::cmd::shared::coerce_value;

    #[test]
    fn param_file_json_merge() {
        let path = std::env::temp_dir().join("mcp_hack_param_test.json");
        // Using a file in the system temp directory instead of the `tempfile` crate.
        std::fs::write(&path, r#"{ "a": 1, "b": "x" }"#).unwrap();
        let mut provided = std::collections::HashMap::new();
        provided.insert("b".into(), "override".into());
        load_param_file_into_map(path.to_str().unwrap(), &mut provided).unwrap();
        assert_eq!(provided.get("a").unwrap(), "1");
        assert_eq!(provided.get("b").unwrap(), "override");
    }

    #[test]
    fn coerce_value_integer_ok() {
        assert_eq!(coerce_value("5", "integer"), serde_json::json!(5));
    }

    #[test]
    fn coerce_value_bool_ok() {
        assert_eq!(coerce_value("yes", "boolean"), serde_json::json!(true));
        assert_eq!(coerce_value("No", "boolean"), serde_json::json!(false));
    }
}
