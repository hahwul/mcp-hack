//! Target parsing (local command vs remote URL).
//!
//! parse_target -> TargetSpec { LocalCommand | RemoteUrl }
//! Helpers: is_local / is_remote / establish (local spawn; remote placeholder).
//! Remote transports not implemented yet.
//!
use anyhow::{Context, Result, bail};
use shell_words::split as shell_split;
use std::fmt;
use tokio::process::Command;
use url::Url;

/// Classification of the high-level target kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    LocalProcess,
    RemoteHttp,
    RemoteWs,
    Unknown,
}

/// A parsed representation of a user-supplied target string.
///
/// It retains the original input for diagnostics and provides
/// structured access to either a remote URL or a local command invocation.
#[derive(Debug, Clone)]
pub enum TargetSpec {
    /// A local process to be spawned. Contains command + arguments.
    LocalCommand {
        original: String,
        program: String,
        args: Vec<String>,
    },
    /// Remote endpoint specified by URL (http/https or ws/wss).
    RemoteUrl { original: String, url: Url },
}

impl TargetSpec {
    /// Returns the original user-supplied form.
    pub fn original(&self) -> &str {
        match self {
            TargetSpec::LocalCommand { original, .. } => original,
            TargetSpec::RemoteUrl { original, .. } => original,
        }
    }

    /// Determine the abstract kind.
    pub fn kind(&self) -> TargetKind {
        match self {
            TargetSpec::LocalCommand { .. } => TargetKind::LocalProcess,
            TargetSpec::RemoteUrl { url, .. } => match url.scheme() {
                "http" | "https" => TargetKind::RemoteHttp,
                "ws" | "wss" => TargetKind::RemoteWs,
                _ => TargetKind::Unknown,
            },
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self.kind(), TargetKind::RemoteHttp | TargetKind::RemoteWs)
    }

    pub fn is_local(&self) -> bool {
        matches!(self.kind(), TargetKind::LocalProcess)
    }
}

impl fmt::Display for TargetSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TargetSpec::LocalCommand { program, args, .. } => {
                if args.is_empty() {
                    write!(f, "local: {}", program)
                } else {
                    write!(f, "local: {} {}", program, args.join(" "))
                }
            }
            TargetSpec::RemoteUrl { url, .. } => write!(f, "remote: {}", url),
        }
    }
}

/// Attempt to parse a `--target` value into a structured `TargetSpec`.
///
/// Parsing Strategy:
/// 1. Try to parse as URL. If successful and scheme ∈ {http, https, ws, wss}, treat as remote.
/// 2. Otherwise treat as a local command line and split with shell-style rules.
/// 3. Reject empty command tokens.
/// 4. Provide contextual errors.
///
/// Examples:
/// - "https://example.org/mcp" -> RemoteUrl
/// - "npx -y @modelcontextprotocol/server-everything" -> LocalCommand
/// - "./my-server --flag" -> LocalCommand
pub fn parse_target(raw: &str) -> Result<TargetSpec> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("Target string is empty");
    }

    if let Ok(url) = Url::parse(trimmed) {
        // Accept only relevant schemes; else fall back to command parsing.
        match url.scheme() {
            "http" | "https" | "ws" | "wss" => {
                return Ok(TargetSpec::RemoteUrl {
                    original: raw.to_string(),
                    url,
                });
            }
            _ => {
                // Non-MCP scheme; fall through to command parsing.
            }
        }
    }

    // Local command path.
    let parts =
        shell_split(trimmed).context("Failed to parse local command line (shell splitting)")?;
    if parts.is_empty() {
        bail!("No tokens produced when parsing local command target");
    }
    let program = parts[0].clone();
    if program.is_empty() {
        bail!("Empty program name in local command target");
    }
    let args = parts[1..].to_vec();
    Ok(TargetSpec::LocalCommand {
        original: raw.to_string(),
        program,
        args,
    })
}

/// Placeholder type representing an established target connection.
///
/// This will evolve to wrap actual RMCP service handles or remote client
/// connections. For now it stores minimal context.
#[derive(Debug)]
pub struct TargetConnection {
    pub spec: TargetSpec,
    pub state: ConnectionState,
}

/// Status of the connection / process.
#[derive(Debug)]
pub enum ConnectionState {
    /// For local processes: we spawned it (future: store child handle / PID).
    LocalSpawned,
    /// For remote endpoints: a session was "logically" established (future: real transport).
    RemotePending,
}

/// Establish (or simulate establishing) a connection to the target.
///
/// Current Behavior:
/// - LocalCommand: spawns the process (without hooking up full MCP transport yet).
/// - RemoteUrl: returns a placeholder pending state.
///
/// Returns a `TargetConnection`.
///
/// NOTE: This function is async to prepare for non-blocking IO + real transports.
/// For local commands we currently spawn the process and detach (placeholder).
pub async fn establish(spec: &TargetSpec) -> Result<TargetConnection> {
    match spec {
        TargetSpec::LocalCommand { program, args, .. } => {
            // Use rmcp transport wrapper to spawn and immediately initialize an MCP service.
            // This replaces the previous raw spawn logic so callers can (soon) reuse
            // the initialized service for tool enumeration / testing.
            use rmcp::{
                ServiceExt,
                transport::{ConfigureCommandExt, TokioChildProcess},
            };

            let service = ()
                .serve(TokioChildProcess::new(Command::new(program).configure(
                    |c| {
                        for a in args {
                            c.arg(a);
                        }
                        // Provide a hint-friendly environment hook (future use).
                        // c.env("MCP_LOG", "info");
                    },
                ))?)
                .await
                .with_context(|| {
                    format!("Failed to spawn & initialize local MCP service: '{}'", spec)
                })?;

            // Basic peer info fetch (debug/logging purpose). Avoids failing if unavailable.
            let _peer_info = service.peer_info();
            eprintln!("[mcp] connected local process: kind={:?}", spec.kind());

            // NOTE: We are not storing `service` inside TargetConnection yet to keep the
            // structure lightweight. Future refactor:
            //   - Extend TargetConnection to hold an Arc<Service<...>>
            //   - Provide graceful shutdown / cancel handling
            Ok(TargetConnection {
                spec: spec.clone(),
                state: ConnectionState::LocalSpawned,
            })
        }
        TargetSpec::RemoteUrl { url, .. } => {
            // Remote URL support (scaffolding):
            // For now we do not fully establish a transport. We:
            //  1. Validate the scheme (http/https/ws/wss already filtered earlier)
            //  2. (Future) If http/https: attempt SSE client connection
            //  3. (Future) If ws/wss: implement websocket transport (feature gated in rmcp)
            //
            // Placeholder behavior: return RemotePending while logging intent.
            eprintln!("[mcp] (scaffold) remote target detected: {}", url);

            // Attempt lightweight validation / normalization for future expansion.
            if url.scheme() == "http" || url.scheme() == "https" {
                // Potential SSE endpoint heuristic:
                // If path doesn't look like an SSE endpoint, we might append '/sse' later.
                // Keep as-is for now.
                // FUTURE:
                // use rmcp::transport::SseClientTransport;
                // let transport = SseClientTransport::start(url.as_str()).await?;
                // let service = ().serve(transport).await?;
            } else if url.scheme() == "ws" || url.scheme() == "wss" {
                // FUTURE:
                // Implement websocket transport once rmcp exposes ws feature again.
            }

            Ok(TargetConnection {
                spec: spec.clone(),
                state: ConnectionState::RemotePending,
            })
        }
    }
}

/// Convenience: parse then establish in one call.
pub async fn parse_and_establish(raw: &str) -> Result<TargetConnection> {
    let spec = parse_target(raw)?;
    establish(&spec).await
}

/// (Scaffold) Establish a remote target connection.
/// For now this delegates to `establish` and returns its result,
/// but provides a semantic placeholder for future remote transport logic.
/// In the future this may:
///  - Negotiate SSE endpoint (http/https)
///  - Perform WebSocket handshake (ws/wss)
///  - Pre-fetch capabilities / tool metadata
pub async fn establish_remote(url: &Url) -> Result<ConnectionState> {
    // Currently we just acknowledge and return pending.
    // Later we will attempt a real transport initialization.
    let _ = url; // suppress unused warning for now
    Ok(ConnectionState::RemotePending)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_remote_http() {
        let spec = parse_target("https://example.com/mcp").unwrap();
        assert!(spec.is_remote());
        assert!(matches!(spec.kind(), TargetKind::RemoteHttp));
    }

    #[test]
    fn parse_remote_ws() {
        let spec = parse_target("wss://mcp.example/ws").unwrap();
        assert!(matches!(spec.kind(), TargetKind::RemoteWs));
    }

    #[test]
    fn parse_local_simple() {
        let spec = parse_target("my-server --flag").unwrap();
        assert!(spec.is_local());
        if let TargetSpec::LocalCommand { program, args, .. } = spec {
            assert_eq!(program, "my-server");
            assert_eq!(args, vec!["--flag"]);
        } else {
            panic!("Expected LocalCommand variant");
        }
    }

    #[test]
    fn parse_local_quoted() {
        let spec = parse_target(r#"my-server --path "/tmp/my dir""#).unwrap();
        if let TargetSpec::LocalCommand { args, .. } = spec {
            assert_eq!(args.len(), 2);
            assert_eq!(args[0], "--path");
            assert_eq!(args[1], "/tmp/my dir");
        } else {
            panic!("Expected LocalCommand variant");
        }
    }

    #[test]
    fn url_with_unknown_scheme_falls_back_to_command() {
        let spec = parse_target("ftp://example.com/resource").unwrap();
        assert!(spec.is_local(), "Unknown scheme should fall back to local");
    }

    #[test]
    fn empty_target_rejected() {
        let err = parse_target("   ").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }
}
