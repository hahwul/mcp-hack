/*!
shared.rs - shared helpers for subcommands.

Focus:
  - fetch_tools_local(_async): spawn local MCP process + list tools
  - extract_tool_array / find_tool_case_insensitive
  - build_arguments_from_schema + primitive coercion
  - summarize_call_result

Goal: keep reusable, minimal logic for list/get/exec. Remote transports,
caching, richer validation left for future iterations.
*/

use anyhow::{Context, Result};
use std::time::Instant;

/* ---- Data Structures ---- */

/// Result of fetching tools from a local MCP target process.
#[derive(Debug)]
pub struct ToolList {
    /// Raw tool objects (each an arbitrary JSON object)
    pub tools: Vec<serde_json::Value>,
    /// Elapsed time (milliseconds) for the entire spawn + enumerate + shutdown flow
    pub elapsed_ms: u128,
}

impl ToolList {
    /// Convenience: number of tools.
    pub fn count(&self) -> usize {
        self.tools.len()
    }

    /// Iterate over raw tool JSON objects.
    pub fn iter(&self) -> impl Iterator<Item = &serde_json::Value> {
        self.tools.iter()
    }
}

/* ---- Fetch / Spawn Helpers ---- */

/// Synchronous convenience wrapper:
///   - Creates a temporary Tokio runtime
///   - Spawns the local MCP server process
///   - Queries available tools
///   - Cancels (graceful shutdown attempt)
///
/// Returns a `ToolList` with raw tool JSON objects.
/// Only supports *local* targets (`TargetSpec::LocalCommand`).
pub fn fetch_tools_local(spec: &crate::mcp::TargetSpec) -> Result<ToolList> {
    let rt = tokio::runtime::Runtime::new().context("Failed to create Tokio runtime")?;
    rt.block_on(fetch_tools_local_async(spec))
}

/// Async variant of tool enumeration for local targets.
pub async fn fetch_tools_local_async(spec: &crate::mcp::TargetSpec) -> Result<ToolList> {
    use rmcp::ServiceExt;
    use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
    use tokio::process::Command;

    let (program, args) = match spec {
        crate::mcp::TargetSpec::LocalCommand { program, args, .. } => {
            (program.clone(), args.clone())
        }
        _ => anyhow::bail!("fetch_tools_local_async only supports local process targets"),
    };

    let started = Instant::now();

    let service = ()
        .serve(TokioChildProcess::new(Command::new(&program).configure(
            |c| {
                for a in &args {
                    c.arg(a);
                }
                // Suppress child stderr (banner / noisy logs) — keep stdout for protocol.
                c.stderr(std::process::Stdio::null());
            },
        ))?)
        .await
        .with_context(|| format!("Failed to spawn MCP process: {}", program))?;

    let tools_resp = service
        .list_tools(Default::default())
        .await
        .context("Failed to list tools from MCP service")?;

    // Attempt graceful shutdown (ignore failure).
    let _ = service.cancel().await;

    let val = serde_json::to_value(&tools_resp).unwrap_or(serde_json::Value::Null);
    let mut tools = Vec::new();
    if let Some(arr) = val.get("tools").and_then(|v| v.as_array()) {
        for t in arr {
            tools.push(t.clone());
        }
    }

    Ok(ToolList {
        tools,
        elapsed_ms: started.elapsed().as_millis(),
    })
}

/* ---- Tool Object Utilities ---- */

/// Return a cloned vector of tool objects from a JSON value containing a `tools` array.
/// Silent on missing / malformed content (returns empty vec).
pub fn extract_tool_array(value: &serde_json::Value) -> Vec<serde_json::Value> {
    value
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|arr| arr.to_vec())
        .unwrap_or_default()
}

/// Find a tool (case-insensitive name match) returning a cloned JSON object.
pub fn find_tool_case_insensitive(
    value: &serde_json::Value,
    name: &str,
) -> Option<serde_json::Value> {
    let arr = value.get("tools")?.as_array()?;
    for t in arr {
        if let Some(n) = t.get("name").and_then(|v| v.as_str())
            && n.eq_ignore_ascii_case(name)
        {
            return Some(t.clone());
        }
    }
    None
}

/* ---- Argument Building / Schema Handling ---- */

/// Build a JSON arguments object based on a tool's `input_schema` / `inputSchema`.
///
/// - `provided` map contains raw string values (from CLI, files, interactive input).
/// - Required detection uses `input_schema.required` (or `inputSchema.required`) array.
/// - Each parameter is coerced according to its declared `"type"` property:
///       integer | number | boolean | array | (default -> string)
/// - Extra keys in `provided` (not in schema) are passed through as strings.
/// - Returns an error if a required parameter is missing.
///
/// NOTE: Strict schema validation (enum constraints, nested objects, etc.) is
/// intentionally deferred for future enhancement.
pub fn build_arguments_from_schema(
    tool_obj: &serde_json::Map<String, serde_json::Value>,
    provided: &std::collections::HashMap<String, String>,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    // Support both snake_case `input_schema` and camelCase `inputSchema`
    let schema = tool_obj
        .get("input_schema")
        .or_else(|| tool_obj.get("inputSchema"))
        .and_then(|v| v.as_object());
    let mut result = serde_json::Map::new();

    // Collect required names
    let mut required: std::collections::HashSet<&str> = std::collections::HashSet::new();
    if let Some(req_arr) = schema
        .and_then(|s| s.get("required"))
        .and_then(|v| v.as_array())
    {
        for r in req_arr {
            if let Some(s) = r.as_str() {
                required.insert(s);
            }
        }
    }

    let mut remaining = provided.clone();

    if let Some(props) = schema
        .and_then(|s| s.get("properties"))
        .and_then(|v| v.as_object())
    {
        for (pname, pobj) in props {
            let ptype = pobj
                .as_object()
                .and_then(|m| m.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("string");
            if let Some(raw_v) = remaining.remove(pname) {
                result.insert(pname.clone(), coerce_value(&raw_v, ptype));
            } else if required.contains(pname.as_str()) {
                anyhow::bail!("missing required parameter: {}", pname);
            }
        }
    }

    // Any leftovers not in schema -> add as simple strings
    for (k, v) in remaining {
        result.insert(k, serde_json::Value::String(v));
    }

    Ok(result)
}

/// Attempt to coerce a raw string into a JSON value using a primitive type hint.
pub fn coerce_value(raw: &str, type_hint: &str) -> serde_json::Value {
    match type_hint {
        "integer" => raw
            .parse::<i64>()
            .map(|n| serde_json::Value::Number(n.into()))
            .unwrap_or_else(|_| serde_json::Value::String(raw.to_string())),
        "number" => raw
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or_else(|| serde_json::Value::String(raw.to_string())),
        "boolean" => {
            let l = raw.to_ascii_lowercase();
            match l.as_str() {
                "true" | "1" | "yes" | "y" => serde_json::Value::Bool(true),
                "false" | "0" | "no" | "n" => serde_json::Value::Bool(false),
                _ => serde_json::Value::String(raw.to_string()),
            }
        }
        "array" => {
            let arr = raw
                .split(',')
                .map(|s| serde_json::Value::String(s.trim().to_string()))
                .collect::<Vec<_>>();
            serde_json::Value::Array(arr)
        }
        // Fallback: treat as plain string
        _ => serde_json::Value::String(raw.to_string()),
    }
}

/* ---- Result Summarization ---- */

/// Convert a `CallToolResult` into JSON for summarization.
/// If serialization fails, returns a small stub object.
pub fn summarize_call_result(call_result: &rmcp::model::CallToolResult) -> serde_json::Value {
    serde_json::to_value(call_result)
        .unwrap_or_else(|_| serde_json::json!({ "note": "unable to serialize result" }))
}

/* ---- Tests (basic) ---- */
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn coerce_integer() {
        assert_eq!(coerce_value("42", "integer"), json!(42));
        assert_eq!(
            coerce_value("x42", "integer"),
            json!("x42"),
            "invalid integer remains string"
        );
    }

    #[test]
    fn coerce_boolean() {
        assert_eq!(coerce_value("true", "boolean"), json!(true));
        assert_eq!(coerce_value("No", "boolean"), json!(false));
        assert_eq!(coerce_value("maybe", "boolean"), json!("maybe"));
    }

    #[test]
    fn coerce_array() {
        assert_eq!(
            coerce_value("a,b, c", "array"),
            json!(["a", "b", "c"]),
            "comma splitting with trimming"
        );
    }

    #[test]
    fn build_arguments_basic() {
        let tool_obj = json!({
            "name":"demo",
            "input_schema":{
                "type":"object",
                "required":["id"],
                "properties":{
                    "id":{"type":"integer","description":"identifier"},
                    "flag":{"type":"boolean"},
                    "tags":{"type":"array"}
                }
            }
        })
        .as_object()
        .cloned()
        .unwrap();

        let mut provided = std::collections::HashMap::new();
        provided.insert("id".into(), "10".into());
        provided.insert("flag".into(), "yes".into());
        provided.insert("tags".into(), "alpha,beta".into());

        let args = build_arguments_from_schema(&tool_obj, &provided).unwrap();
        assert_eq!(args.get("id"), Some(&json!(10)));
        assert_eq!(args.get("flag"), Some(&json!(true)));
        assert_eq!(args.get("tags"), Some(&json!(["alpha", "beta"])));
    }

    #[test]
    fn build_arguments_missing_required() {
        let tool_obj = json!({
            "name":"demo",
            "input_schema":{
                "type":"object",
                "required":["id"],
                "properties":{
                    "id":{"type":"integer"}
                }
            }
        })
        .as_object()
        .cloned()
        .unwrap();

        let provided = std::collections::HashMap::<String, String>::new();
        let err = build_arguments_from_schema(&tool_obj, &provided).unwrap_err();
        assert!(
            err.to_string().contains("missing required parameter"),
            "expected required parameter error"
        );
    }

    #[test]
    fn extract_tool_array_empty() {
        let val = json!({"tools":[]});
        let list = extract_tool_array(&val);
        assert!(list.is_empty());
    }

    #[test]
    fn find_tool_case_insensitive_works() {
        let val = json!({"tools":[{"name":"Alpha"},{"name":"beta"}]});
        let t = find_tool_case_insensitive(&val, "ALPHA").unwrap();
        assert_eq!(t.get("name").and_then(|v| v.as_str()), Some("Alpha"));
    }
}
