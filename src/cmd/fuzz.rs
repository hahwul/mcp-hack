/*!
fuzz.rs - fuzz subcommand.

Iterates through a wordlist, substituting a placeholder in parameters,
and invokes an MCP tool for each variation. This is useful for basic
fuzzing and enumeration tasks.

Example:
  mcp fuzz tool "file.read" -p "path=FUZZ" -w /usr/share/wordlists/common.txt

*/

use anyhow::{Context, Result};
use clap::Args;
use std::fs::File;
use std::io::{self, BufRead};
use std::time::Instant;

use super::subject::Subject;
use crate::cmd::exec::{invoke_tool, load_param_file_into_map, output_error};
use crate::cmd::format::{Role, StyleOptions, color, emoji};
use crate::cmd::shared::summarize_call_result;
use crate::mcp;

/* ---- Argument Struct ---- */

#[derive(Args, Debug)]
pub struct FuzzArgs {
    /// Subject to execute ('tool' only)
    pub subject: Subject,

    /// Tool name to invoke
    #[arg(value_name = "TOOL")]
    pub tool: String,

    /// Path to the wordlist file
    #[arg(short = 'w', long, value_name = "PATH")]
    pub wordlist: String,

    /// Placeholder string in parameters to replace (default: FUZZ)
    #[arg(short = 'p', long, value_name = "STRING", default_value = "FUZZ")]
    pub placeholder: String,

    /// Provide parameter (KEY=VALUE), repeatable. Use placeholder for substitution.
    #[arg(long = "param", value_name = "KEY=VALUE")]
    pub params: Vec<String>,

    /// Load parameters from file (JSON or YAML). CLI --param overrides file entries.
    #[arg(long = "param-file", value_name = "PATH")]
    pub param_file: Option<String>,

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

/* ---- Public Entry Point ---- */

pub fn execute_fuzz(mut args: FuzzArgs) -> Result<()> {
    // Subject check
    if !matches!(args.subject, Subject::Tool) {
        return output_error(args.json, "fuzz currently supports only subject 'tool'");
    }

    // Tool name validation
    let tool_name_owned = args.tool.trim().to_string();
    if tool_name_owned.is_empty() {
        return output_error(args.json, "tool name cannot be empty");
    }

    // Determine target (CLI > env)
    if args.target.is_none()
        && let Ok(env_t) = std::env::var("MCP_TARGET")
            && !env_t.trim().is_empty() {
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
        .with_context(|| format!("Failed to parse target: '{}'", target_raw))?;

    if !spec.is_local() {
        return output_error(args.json, "remote fuzz not implemented yet");
    }

    // --- Fuzzing-specific logic starts here ---

    // Read wordlist
    let wordlist_path = &args.wordlist;
    let file = File::open(wordlist_path)
        .with_context(|| format!("Failed to open wordlist file: {}", wordlist_path))?;
    let reader = io::BufReader::new(file);
    let words: Vec<String> = reader.lines().collect::<Result<_, _>>()?;
    let total_requests = words.len();

    if !args.json {
        let style = StyleOptions::detect();
        println!(
            "{} {}",
            emoji("info", &style),
            color(
                Role::Accent,
                format!(
                    "Starting fuzz session: {} requests for tool '{}'",
                    total_requests, tool_name_owned
                ),
                &style
            )
        );
    }

    // Loop through wordlist and execute
    for (i, word) in words.iter().enumerate() {
        let mut provided: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        // Collect parameters from CLI, substituting the placeholder
        for kv in &args.params {
            let substituted_kv = kv.replace(&args.placeholder, word);
            if let Some((k, v)) = substituted_kv.split_once('=') {
                let key = k.trim();
                if key.is_empty() {
                    return output_error(
                        args.json,
                        &format!("invalid --param (empty key): {}", kv),
                    );
                }
                provided.insert(key.to_string(), v.trim().to_string());
            } else {
                return output_error(
                    args.json,
                    &format!("invalid --param (expected KEY=VALUE): {}", kv),
                );
            }
        }

        // Load param file if specified (merge non-conflicting keys)
        if let Some(ref pf) = args.param_file
            && let Err(e) = load_param_file_into_map(pf, &mut provided) {
                return output_error(args.json, &e.to_string());
            }

        // Build runtime + spawn + list tools + call tool
        let started = Instant::now();
        let result = invoke_tool(
            &spec,
            &tool_name_owned,
            provided,
            false, // Interactive mode is disabled for fuzzing
            args.json,
        );
        let elapsed_ms = started.elapsed().as_millis();

        match result {
            Ok((final_args_map, call_result)) => {
                if args.json {
                    let mut base = serde_json::json!({
                        "status": "ok",
                        "request_index": i,
                        "total_requests": total_requests,
                        "word": word,
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
                                    .unwrap_or_else(|_| serde_json::json!({"error": "serialize"})),
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
                        serde_json::to_string(&base).unwrap_or_else(|_| base.to_string())
                    );
                } else {
                    let style = StyleOptions::detect();
                    let summary = summarize_call_result(&call_result);
                    let summary_str =
                        serde_json::to_string(&summary).unwrap_or_else(|_| summary.to_string());

                    println!(
                        "{} Request {}/{}: word='{}' -> {}",
                        emoji("success", &style),
                        i + 1,
                        total_requests,
                        word,
                        summary_str
                    );
                }
            }
            Err(e) => {
                if args.json {
                    let err = serde_json::json!({
                        "status": "error",
                        "request_index": i,
                        "total_requests": total_requests,
                        "word": word,
                        "error": e.to_string()
                    });
                    println!(
                        "{}",
                        serde_json::to_string(&err).unwrap_or_else(|_| err.to_string())
                    );
                } else {
                    let style = StyleOptions::detect();
                    println!(
                        "{} Request {}/{}: word='{}' -> {}",
                        emoji("error", &style),
                        i + 1,
                        total_requests,
                        word,
                        color(Role::Error, e.to_string(), &style)
                    );
                }
            }
        }
    }

    Ok(())
}
