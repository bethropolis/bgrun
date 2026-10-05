//! Stateless stdio MCP (Model Context Protocol) server.
//!
//! `bgrun mcp` speaks JSON-RPC 2.0 on stdin/stdout (newline-delimited frames)
//! and exposes the daemon's operations as MCP tools. It holds no state: each
//! `tools/call` opens a fresh daemon connection (auto-starting the daemon when
//! needed), so the server can be restarted at will and never blocks on
//! long-lived streams. Interactive commands (`attach`, log streaming) are
//! intentionally not exposed — MCP clients should poll `diff` instead.

use anyhow::Result;
use bgrun_proto::{Command, KillArgs, RunArgs, TailArgs};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::mcp_types::{CallToolResult, JsonRpcRequest, JsonRpcResponse, McpTool};

/// Protocol versions this server can speak (newest first). `initialize`
/// requests with a supported version are answered in kind; anything else
/// gets the newest one (the client decides compatibility).
const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Runs the stdio MCP server until stdin is closed. Only JSON-RPC frames are
/// ever written to stdout; diagnostics never go there (stdout is the wire).
pub async fn run() -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut frames = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = frames.next_line().await? {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(request) => handle_request(request).await,
            // A frame we cannot parse gets id:null per JSON-RPC 2.0.
            Err(error) => Some(JsonRpcResponse::err(
                Some(Value::Null),
                -32700,
                format!("parse error: {error}"),
            )),
        };
        if let Some(response) = response {
            write_frame(&mut stdout, &response).await?;
        }
    }
    Ok(())
}

/// Writes one NDJSON frame and flushes.
async fn write_frame(stdout: &mut tokio::io::Stdout, response: &JsonRpcResponse) -> Result<()> {
    let mut frame = serde_json::to_string(response)?;
    frame.push('\n');
    stdout.write_all(frame.as_bytes()).await?;
    stdout.flush().await?;
    Ok(())
}

/// Dispatches one JSON-RPC request. Returns `None` for notifications (no id),
/// which per spec must not produce a response.
async fn handle_request(request: JsonRpcRequest) -> Option<JsonRpcResponse> {
    let JsonRpcRequest {
        jsonrpc,
        id,
        method,
        params,
    } = request;
    let Some(id) = id else {
        return None; // notification: observe, never answer
    };
    if jsonrpc != "2.0" {
        return Some(JsonRpcResponse::err(
            Some(id),
            -32600,
            format!("unsupported jsonrpc version: {jsonrpc}"),
        ));
    }

    match method.as_str() {
        "initialize" => Some(initialize(id, params.as_ref())),
        "ping" => Some(JsonRpcResponse::ok(Some(id), json!({}))),
        "tools/list" => Some(JsonRpcResponse::ok(
            Some(id),
            json!({ "tools": tool_registry() }),
        )),
        "tools/call" => Some(call_tool(id, params).await),
        // Normally a notification (caught by the `id` guard above); answer
        // defensively if a client sends it as a request.
        "initialized" | "notifications/initialized" => {
            Some(JsonRpcResponse::ok(Some(id), json!({})))
        }
        _ => Some(JsonRpcResponse::err(
            Some(id),
            -32601,
            format!("method not found: {method}"),
        )),
    }
}

/// Answers `initialize` with the negotiated protocol version and capabilities.
fn initialize(id: Value, params: Option<&Value>) -> JsonRpcResponse {
    let requested = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str);
    let protocol_version = requested
        .filter(|version| SUPPORTED_PROTOCOL_VERSIONS.contains(version))
        .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);

    JsonRpcResponse::ok(
        Some(id),
        json!({
            "protocolVersion": protocol_version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "bgrun",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions": "Manage long-running background jobs: start them (run), poll output incrementally (diff) or read history (tail), wait for readiness (wait), inspect resources (stats), and stop or kill them. Job `id` accepts either the job ID or its --name handle. The daemon is started automatically on first use.",
        }),
    )
}

/// Handles `tools/call`: maps the tool name + arguments to a daemon command
/// and executes it. Unknown tools / bad arguments are protocol errors
/// (-32602); daemon-side failures become `isError` tool results.
async fn call_tool(id: Value, params: Option<Value>) -> JsonRpcResponse {
    let Some(name) = params
        .as_ref()
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
    else {
        return JsonRpcResponse::err(Some(id), -32602, "tools/call requires a tool name");
    };
    let arguments = params
        .as_ref()
        .and_then(|p| p.get("arguments").cloned())
        .unwrap_or_else(|| json!({}));

    let command = match build_command(name, &arguments) {
        Ok(command) => command,
        Err(message) => return JsonRpcResponse::err(Some(id), -32602, message),
    };

    let result = execute(command).await;
    JsonRpcResponse::ok(
        Some(id),
        serde_json::to_value(&result).unwrap_or_else(|_| {
            json!({ "content": [{ "type": "text", "text": "result serialization failed" }], "isError": true })
        }),
    )
}

/// Executes one daemon command on a fresh connection (stateless per call).
async fn execute(command: Command) -> CallToolResult {
    let socket_path = bgrun_proto::paths::socket_path();
    if let Err(error) = ensure_daemon_running(&socket_path).await {
        return CallToolResult::error(format!("daemon unavailable: {error:#}"));
    }

    let mut client = match DaemonClient::connect(&socket_path).await {
        Ok(client) => client,
        Err(error) => {
            return CallToolResult::error(format!("failed to connect to daemon: {error:#}"));
        }
    };

    let response = match client.send::<Value>(command).await {
        Ok(response) => response,
        Err(error) => return CallToolResult::error(format!("daemon request failed: {error:#}")),
    };

    if !response.ok {
        return CallToolResult::error(
            response
                .error
                .unwrap_or_else(|| "unknown daemon error".to_string()),
        );
    }

    let text = match &response.data {
        Some(data) => serde_json::to_string_pretty(data).unwrap_or_else(|_| "ok".to_string()),
        None => "ok".to_string(),
    };
    CallToolResult::success(text)
}

// ---------------------------------------------------------------------------
// Tool registry
// ---------------------------------------------------------------------------

/// The MCP tool table: one entry per non-interactive daemon command.
///
/// Keep in sync with `Command` in bgrun-proto; the
/// `every_command_variant_maps_to_a_tool` test in this module fails when a
/// variant is added without a mapping here.
pub fn tool_registry() -> Vec<McpTool> {
    vec![
        tool(
            "run",
            "Start a background job and return its record (id, name, pid, state). \
             `cmd` is the command vector; single-word `cmd` values are also resolved \
             against bgrun.toml jobs. Optional: `name` (stable handle), `workspace`, \
             `readiness` ({\"log_pattern\": \"...\"} | {\"log_pattern_regex\": \"...\"} | \
             {\"http_poll\": \"url\"} | {\"tcp_port\": 8080} | {\"file_exists\": \"path\"}), \
             `restart` (Never | OnCrash/OnFailure/Always with backoff_ms), resource caps \
             (`memory_max` bytes, `cpu_quota` %, `pids_max`), `env` map, `cwd`, `pty`, \
             `sandbox`, `allocate_port` (env var name), health-check fields.",
            serde_json::to_value(schemars::schema_for!(RunArgs))
                .unwrap_or_else(|_| json!({ "type": "object" })),
        ),
        tool(
            "list",
            "List all jobs with id, name, workspace, state, pid, port, and command.",
            json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "string", "description": "Only list jobs in this workspace." }
                }
            }),
        ),
        tool(
            "status",
            "Get a job's state (starting/running/ready/exited/crashed/killed), exit code, \
             readiness timestamp, and restart counters.",
            id_object(None),
        ),
        tool(
            "kill",
            "Kill a job (SIGKILL), or all jobs in a workspace with `workspace` instead of `id`.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Job ID or name (unless `workspace` is given)." },
                    "workspace": { "type": "string", "description": "Kill every job in this workspace instead." }
                }
            }),
        ),
        tool(
            "stop",
            "Gracefully stop a job: SIGTERM first, escalating to SIGKILL after \
             `timeout_ms` (default 10000). Also marks already-finished jobs as killed \
             so pending restart policies are suppressed.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Job ID or name." },
                    "timeout_ms": { "type": "integer", "description": "Grace period before SIGKILL (default 10000)." }
                },
                "required": ["id"]
            }),
        ),
        tool(
            "restart",
            "Stop the job if alive (same semantics as stop), then re-spawn it from its \
             stored definition and return the new record.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Job ID or name." },
                    "timeout_ms": { "type": "integer", "description": "Grace period before SIGKILL (default 10000)." }
                },
                "required": ["id"]
            }),
        ),
        tool(
            "tail",
            "Read the last `lines` (default 50) lines of a job's log, optionally filtered \
             by `level` substring (e.g. \"error\"), `stream` (stdout/stderr/pty) or \
             `filter_regex`. With `digest: true`, returns a summary instead \
             (total lines, error/warning counts, last error).",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Job ID or name." },
                    "lines": { "type": "integer", "description": "How many lines to return (default 50)." },
                    "digest": { "type": "boolean", "description": "Return an error/warning digest instead of raw lines." },
                    "level": { "type": "string", "description": "Substring level filter (e.g. \"error\", \"warn\")." },
                    "stream": { "type": "string", "description": "Filter by stream: \"stdout\", \"stderr\", or \"pty\"." },
                    "strip_ansi": { "type": "boolean", "description": "Strip ANSI escape codes from content." },
                    "filter_regex": { "type": "string", "description": "Only return lines matching this regex." }
                },
                "required": ["id"]
            }),
        ),
        tool(
            "diff",
            "Return only the log lines added since the previous diff call for this job \
             (incremental polling with a per-job cursor). Supports the same `stream`, \
             and `filter_regex` filters as tail.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Job ID or name." },
                    "lines": { "type": "integer", "description": "Cap the number of returned lines." },
                    "strip_ansi": { "type": "boolean", "description": "Strip ANSI escape codes from content." },
                    "stream": { "type": "string", "description": "Filter by stream: \"stdout\", \"stderr\", or \"pty\"." },
                    "filter_regex": { "type": "string", "description": "Only return lines matching this regex." }
                },
                "required": ["id"]
            }),
        ),
        tool(
            "wait",
            "Block until the job becomes ready, reaches a terminal state, or `timeout_ms` \
             elapses (default 60000). Returns ready/elapsed/exit code/state.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Job ID or name." },
                    "timeout_ms": { "type": "integer", "description": "Maximum wait (default 60000)." }
                },
                "required": ["id"]
            }),
        ),
        tool(
            "send",
            "Write data to a PTY job's stdin. Append \"\\n\" to the data (or send just a \
             newline) to simulate pressing Enter.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Job ID or name." },
                    "data": { "type": "string", "description": "Bytes to write to the job's stdin." }
                },
                "required": ["id", "data"]
            }),
        ),
        tool(
            "stats",
            "Get resource stats (CPU %, RSS MB, uptime seconds) for a running job.",
            id_object(None),
        ),
        tool(
            "run_group",
            "Start several jobs at once; each entry has the same shape as the `run` tool's arguments.",
            json!({
                "type": "object",
                "properties": {
                    "jobs": {
                        "type": "array",
                        "items": {},
                        "description": "RunArgs entries, as accepted by the `run` tool."
                    }
                },
                "required": ["jobs"]
            }),
        ),
        tool(
            "expect",
            "Block until `pattern` appears in the job's log (plain substring by default, \
             regex with `is_regex: true`), or `timeout_ms` elapses (default 60000).",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Job ID or name." },
                    "pattern": { "type": "string", "description": "Substring or regex to wait for." },
                    "is_regex": { "type": "boolean", "description": "Treat pattern as a regex (default false)." },
                    "timeout_ms": { "type": "integer", "description": "Maximum wait (default 60000)." }
                },
                "required": ["id", "pattern"]
            }),
        ),
        tool(
            "screen",
            "Read the last `lines` (default 20) rendered rows from a PTY job's in-memory \
             screen buffer without touching the log file.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Job ID or name." },
                    "lines": { "type": "integer", "description": "How many rows to return (default 20)." }
                },
                "required": ["id"]
            }),
        ),
        tool(
            "clean",
            "Remove all terminal-state (crashed/exited/killed) jobs, optionally scoped \
             to a workspace.",
            json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "string", "description": "Only clean jobs in this workspace." }
                }
            }),
        ),
    ]
}

/// Builds a minimal one-property schema requiring `id`.
fn id_object(id_description: Option<&str>) -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": id_description.unwrap_or("Job ID or name.") }
        },
        "required": ["id"]
    })
}

fn tool(name: &str, description: &str, input_schema: Value) -> McpTool {
    McpTool {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
    }
}

// ---------------------------------------------------------------------------
// Tool → Command mapping
// ---------------------------------------------------------------------------

/// Maps an MCP tool name plus raw JSON arguments to a daemon `Command`.
/// Errors are user-facing (returned as JSON-RPC -32602).
fn build_command(name: &str, arguments: &Value) -> std::result::Result<Command, String> {
    match name {
        "run" => {
            let args: RunArgs = serde_json::from_value(arguments.clone())
                .map_err(|error| format!("invalid arguments for run: {error}"))?;
            Ok(Command::Run(args))
        }
        "list" => Ok(Command::List {
            workspace: get_str(arguments, "workspace"),
        }),
        "status" => Ok(Command::Status {
            id: require_str(arguments, "id")?,
        }),
        "kill" => {
            let args: KillArgs = serde_json::from_value(arguments.clone())
                .map_err(|error| format!("invalid arguments for kill: {error}"))?;
            Ok(Command::Kill(args))
        }
        "stop" => Ok(Command::Stop {
            id: require_str(arguments, "id")?,
            timeout_ms: get_u64(arguments, "timeout_ms", 10_000)?,
        }),
        "restart" => Ok(Command::Restart {
            id: require_str(arguments, "id")?,
            timeout_ms: get_u64(arguments, "timeout_ms", 10_000)?,
        }),
        "tail" => Ok(Command::Tail(TailArgs {
            id: require_str(arguments, "id")?,
            lines: get_usize(arguments, "lines", 50)?,
            digest: get_bool(arguments, "digest", false),
            level: get_str(arguments, "level"),
            strip_ansi: get_bool(arguments, "strip_ansi", false),
            stream: get_str(arguments, "stream"),
            cursor: None,
            follow: false,
            filter_regex: get_str(arguments, "filter_regex"),
        })),
        "diff" => Ok(Command::Diff {
            id: require_str(arguments, "id")?,
            lines: get_usize_opt(arguments, "lines")?,
            strip_ansi: get_bool(arguments, "strip_ansi", false),
            stream: get_str(arguments, "stream"),
            filter_regex: get_str(arguments, "filter_regex"),
        }),
        "wait" => Ok(Command::Wait {
            id: require_str(arguments, "id")?,
            timeout_ms: get_u64(arguments, "timeout_ms", 60_000)?,
        }),
        "send" => Ok(Command::Send {
            id: require_str(arguments, "id")?,
            data: require_str(arguments, "data")?,
        }),
        "stats" => Ok(Command::Stats {
            id: require_str(arguments, "id")?,
        }),
        "run_group" => {
            let jobs: Vec<RunArgs> =
                serde_json::from_value(arguments.get("jobs").cloned().unwrap_or_else(|| json!([])))
                    .map_err(|error| format!("invalid jobs for run_group: {error}"))?;
            Ok(Command::RunGroup { jobs })
        }
        "expect" => Ok(Command::Expect {
            id: require_str(arguments, "id")?,
            pattern: require_str(arguments, "pattern")?,
            is_regex: get_bool(arguments, "is_regex", false),
            timeout_ms: get_u64(arguments, "timeout_ms", 60_000)?,
        }),
        "screen" => Ok(Command::Screen {
            id: require_str(arguments, "id")?,
            lines: get_usize(arguments, "lines", 20)?,
        }),
        "clean" => Ok(Command::Clean {
            workspace: get_str(arguments, "workspace"),
        }),
        other => Err(format!("unknown tool: {other}")),
    }
}

/// Reads an optional string property.
fn get_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .filter(|value| !value.is_null())
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// Reads a required, non-empty string property.
fn require_str(args: &Value, key: &str) -> std::result::Result<String, String> {
    get_str(args, key)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing required argument: {key}"))
}

/// Reads an optional bool property with a default.
fn get_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

/// Reads an optional u64 property with a default.
fn get_u64(args: &Value, key: &str, default: u64) -> std::result::Result<u64, String> {
    match args.get(key).filter(|value| !value.is_null()) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .ok_or_else(|| format!("{key} must be a non-negative integer")),
    }
}

/// Reads an optional usize property with a default.
fn get_usize(args: &Value, key: &str, default: usize) -> std::result::Result<usize, String> {
    match args.get(key).filter(|value| !value.is_null()) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| format!("{key} must be a non-negative integer")),
    }
}

/// Reads an optional usize property without a default.
fn get_usize_opt(args: &Value, key: &str) -> std::result::Result<Option<usize>, String> {
    match args.get(key).filter(|value| !value.is_null()) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(|n| n as usize)
            .map(Some)
            .ok_or_else(|| format!("{key} must be a non-negative integer")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bgrun_proto::Response;

    /// Sample `RunArgs` covering every field, so the roundtrip test doubles as
    /// a smoke test for the `run` tool's generated schema input.
    fn sample_run_args() -> RunArgs {
        RunArgs {
            cmd: vec!["cargo".into(), "run".into()],
            name: Some("dev".into()),
            workspace: Some("ws".into()),
            readiness: Some(bgrun_proto::ReadinessStrategy::TcpPort(8080)),
            restart: Some(bgrun_proto::RestartPolicy::OnCrash { backoff_ms: 500 }),
            max_retries: Some(3),
            log_max_size: Some(1024),
            log_keep: Some(1),
            memory_max: Some(512),
            cpu_quota: Some(50),
            pids_max: Some(64),
            sandbox: false,
            pty: false,
            max_runtime_ms: Some(1000),
            env: [("KEY".to_string(), "VAL".to_string())]
                .into_iter()
                .collect(),
            after: None,
            cwd: None,
            pty_cols: None,
            pty_rows: None,
            max_rss_mb: None,
            allocate_port: None,
            health_check: None,
            health_interval_secs: None,
            health_threshold: None,
        }
    }

    #[test]
    fn tool_registry_is_wellformed() {
        let tools = tool_registry();
        assert_eq!(tools.len(), 15, "registry must stay in sync with Command");
        let mut seen = std::collections::HashSet::new();
        for tool in &tools {
            assert!(!tool.name.is_empty());
            assert!(!tool.description.is_empty(), "tool {}", tool.name);
            assert_eq!(
                tool.input_schema.get("type").and_then(Value::as_str),
                Some("object"),
                "tool {} schema",
                tool.name
            );
            assert!(
                seen.insert(tool.name.as_str()),
                "duplicate tool {}",
                tool.name
            );
        }
    }

    #[test]
    fn build_command_maps_arguments() {
        let command = build_command("stop", &json!({ "id": "web" })).unwrap();
        assert_eq!(
            command,
            Command::Stop {
                id: "web".into(),
                timeout_ms: 10_000
            }
        );

        let command = build_command(
            "tail",
            &json!({ "id": "web", "lines": 5, "level": "error" }),
        )
        .unwrap();
        assert_eq!(
            command,
            Command::Tail(TailArgs {
                id: "web".into(),
                lines: 5,
                digest: false,
                level: Some("error".into()),
                strip_ansi: false,
                stream: None,
                cursor: None,
                follow: false,
                filter_regex: None,
            })
        );

        let command = build_command("diff", &json!({ "id": "web", "strip_ansi": true })).unwrap();
        assert_eq!(
            command,
            Command::Diff {
                id: "web".into(),
                lines: None,
                strip_ansi: true,
                stream: None,
                filter_regex: None,
            }
        );
    }

    #[test]
    fn build_command_rejects_bad_input() {
        let error = build_command("stop", &json!({})).unwrap_err();
        assert!(error.contains("missing required argument: id"), "{error}");
        let error = build_command("nope", &json!({})).unwrap_err();
        assert!(error.contains("unknown tool"), "{error}");
        let error = build_command("wait", &json!({ "id": "x", "timeout_ms": -1 })).unwrap_err();
        assert!(error.contains("non-negative"), "{error}");
        // `run` surfaces serde errors for malformed payloads.
        let error = build_command("run", &json!({ "cmd": "not-a-list" })).unwrap_err();
        assert!(error.contains("invalid arguments for run"), "{error}");
    }

    #[test]
    fn build_command_run_roundtrips() {
        let args = sample_run_args();
        let json_args = serde_json::to_value(&args).unwrap();
        let command = build_command("run", &json_args).unwrap();
        assert_eq!(command, Command::Run(args));
    }

    #[test]
    fn build_command_run_accepts_job_name_resolution_input() {
        // Single-word cmd values resolve against bgrun.toml jobs; the MCP
        // mapper must pass them through untouched. Partial payloads are fine
        // thanks to the serde defaults on RunArgs.
        let command = build_command("run", &json!({ "cmd": ["dev"] })).unwrap();
        let expected: RunArgs = serde_json::from_value(json!({ "cmd": ["dev"] })).unwrap();
        assert_eq!(command, Command::Run(expected));
    }

    #[test]
    fn build_command_kill_and_clean() {
        let command = build_command("kill", &json!({ "workspace": "ws" })).unwrap();
        assert_eq!(
            command,
            Command::Kill(KillArgs {
                id: None,
                workspace: Some("ws".into())
            })
        );
        let command = build_command("clean", &json!({})).unwrap();
        assert_eq!(command, Command::Clean { workspace: None });
        let command = build_command("send", &json!({ "id": "x", "data": "hi\n" })).unwrap();
        assert_eq!(
            command,
            Command::Send {
                id: "x".into(),
                data: "hi\n".into()
            }
        );
    }

    /// The daemon error envelope maps onto `isError` tool results (mirrors the
    /// branch `execute()` takes for `Response.ok == false`).
    #[test]
    fn daemon_error_envelope_shape() {
        let response: Response<Value> = Response::err("req".into(), "job not found");
        assert!(!response.ok);
        assert_eq!(response.error.as_deref(), Some("job not found"));
        // Same mapping execute() applies:
        let text = response
            .error
            .unwrap_or_else(|| "unknown daemon error".to_string());
        let result = CallToolResult::error(text);
        let value = serde_json::to_value(&result).unwrap();
        assert_eq!(value["isError"], true);
        assert_eq!(value["content"][0]["text"], "job not found");
    }

    /// Exhaustive mapping check: every non-hijacking `Command` variant must
    /// have a registered tool. Adding a variant breaks compilation here until
    /// a tool and mapping are added — the CI guard against schema drift.
    /// `Attach`, `ResizePty`, and `StreamLogs` belong to interactive sessions
    /// and are intentionally absent; `Export` is a local filesystem
    /// convenience, also not surfaced.
    #[test]
    fn every_command_variant_maps_to_a_tool() {
        let commands: Vec<Command> = vec![
            Command::Run(sample_run_args()),
            Command::Status { id: "x".into() },
            Command::List { workspace: None },
            Command::Kill(KillArgs {
                id: Some("x".into()),
                workspace: None,
            }),
            Command::Stop {
                id: "x".into(),
                timeout_ms: 1000,
            },
            Command::Restart {
                id: "x".into(),
                timeout_ms: 1000,
            },
            Command::Tail(TailArgs {
                id: "x".into(),
                lines: 10,
                digest: false,
                level: None,
                strip_ansi: false,
                stream: None,
                cursor: None,
                follow: false,
                filter_regex: None,
            }),
            Command::Diff {
                id: "x".into(),
                lines: None,
                strip_ansi: false,
                stream: None,
                filter_regex: None,
            },
            Command::Wait {
                id: "x".into(),
                timeout_ms: 1000,
            },
            Command::Send {
                id: "x".into(),
                data: "y".into(),
            },
            Command::Stats { id: "x".into() },
            Command::RunGroup {
                jobs: vec![sample_run_args()],
            },
            Command::Expect {
                id: "x".into(),
                pattern: "y".into(),
                is_regex: false,
                timeout_ms: 1000,
            },
            Command::Screen {
                id: "x".into(),
                lines: 20,
            },
            Command::Clean { workspace: None },
            // Intentionally not MCP tools:
            Command::Attach { id: "x".into() },
            Command::ResizePty {
                id: "x".into(),
                cols: 80,
                rows: 24,
            },
            Command::Export {
                id: "x".into(),
                path: "p".into(),
                lines: None,
                level: None,
                stream: None,
                strip_ansi: false,
                filter_regex: None,
                since_ms: None,
            },
            Command::StreamLogs {
                id: "x".into(),
                cursor: None,
                level: None,
                filter_regex: None,
            },
        ];

        let tools = tool_registry();
        assert_eq!(tools.len(), 15);
        for command in &commands {
            match command {
                Command::Run(_) => assert_tool(&tools, "run"),
                Command::Status { .. } => assert_tool(&tools, "status"),
                Command::List { .. } => assert_tool(&tools, "list"),
                Command::Kill(_) => assert_tool(&tools, "kill"),
                Command::Stop { .. } => assert_tool(&tools, "stop"),
                Command::Restart { .. } => assert_tool(&tools, "restart"),
                Command::Tail(_) => assert_tool(&tools, "tail"),
                Command::Diff { .. } => assert_tool(&tools, "diff"),
                Command::Wait { .. } => assert_tool(&tools, "wait"),
                Command::Send { .. } => assert_tool(&tools, "send"),
                Command::Stats { .. } => assert_tool(&tools, "stats"),
                Command::RunGroup { .. } => assert_tool(&tools, "run_group"),
                Command::Expect { .. } => assert_tool(&tools, "expect"),
                Command::Screen { .. } => assert_tool(&tools, "screen"),
                Command::Clean { .. } => assert_tool(&tools, "clean"),
                // Deliberately not exposed as MCP tools:
                // - `Attach`/`StreamLogs` need a hijacked, long-lived
                //   connection; MCP clients should poll `diff` instead.
                // - `ResizePty` only matters while attached.
                // - `Export` writes to an arbitrary caller-supplied path on
                //   the host filesystem. Exposing it would let a tool call
                //   write anywhere the user can, so it stays CLI-only.
                Command::Attach { .. }
                | Command::ResizePty { .. }
                | Command::Export { .. }
                | Command::StreamLogs { .. } => {}
            }
        }
    }

    fn assert_tool(tools: &[McpTool], name: &str) {
        assert!(
            tools.iter().any(|tool| tool.name == name),
            "no MCP tool for Command::{name}"
        );
    }
}
