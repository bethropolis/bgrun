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

    let result = execute(name, command, &arguments).await;
    JsonRpcResponse::ok(
        Some(id),
        serde_json::to_value(&result).unwrap_or_else(|_| {
            json!({ "content": [{ "type": "text", "text": "result serialization failed" }], "isError": true })
        }),
    )
}

/// Token budget for a single tool's rendered output. Payloads that exceed it
/// are cut with an explicit `[truncated ...]` marker telling the caller how
/// to re-query narrower, so output is never silently incomplete.
const MAX_TOOL_TEXT_BYTES: usize = 65_536;

/// Upper bound on lines returned per log-reading call. Unbounded `lines`
/// values can flood the caller's context window in one shot.
const MAX_TOOL_LINES: usize = 1000;

/// Executes one daemon command on a fresh connection (stateless per call),
/// then shapes the result for token efficiency: compact JSON, a trimmed
/// `list` view unless `verbose` was requested, and a byte ceiling with an
/// explicit truncation marker.
///
/// `tool_name` and `arguments` are the original MCP call, used only for
/// output shaping (`list` verbosity) and follow-up behavior (`run` + `wait`).
async fn execute(tool_name: &str, command: Command, arguments: &Value) -> CallToolResult {
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

    let query_id = command_job_id(&command);
    let response = match client.send::<Value>(command).await {
        Ok(response) => response,
        Err(error) => return CallToolResult::error(format!("daemon request failed: {error:#}")),
    };

    if !response.ok {
        let message = response
            .error
            .unwrap_or_else(|| "unknown daemon error".to_string());
        return CallToolResult::error(
            suggest_similar_jobs(&message, query_id.as_deref(), &mut client).await,
        );
    }

    let mut data = response.data.unwrap_or(Value::Null);

    // `run` with `wait: true` folds the usual run-then-wait round trip into
    // one call. The wait only starts after a successful spawn.
    if tool_name == "run" && get_bool(arguments, "wait", false) {
        match data.get("id").and_then(Value::as_str).map(str::to_owned) {
            Some(job_id) => {
                let timeout_ms = get_u64(arguments, "wait_timeout_ms", 60_000).unwrap_or(60_000);
                match execute_value(
                    &mut client,
                    Command::Wait {
                        id: job_id.clone(),
                        timeout_ms,
                    },
                )
                .await
                {
                    Ok(waited) => {
                        data = json!({ "run": data, "wait": waited });
                    }
                    Err(error) => {
                        return CallToolResult::error(format!(
                            "started job {job_id}, but waiting failed: {error}"
                        ));
                    }
                }
            }
            None => {
                return CallToolResult::error(
                    "started job, but the daemon returned no job id to wait on",
                );
            }
        }
    }

    if tool_name == "list" && !get_bool(arguments, "verbose", false) {
        data = trim_job_list(&data);
    }

    let text = serde_json::to_string(&data).unwrap_or_else(|_| "ok".to_string());
    CallToolResult::success(apply_text_ceiling(tool_name, text))
}

/// Sends one command over an established connection, returning the payload.
/// Errors are plain strings so follow-up calls (e.g. `run` + `wait`) can
/// compose them into a single tool result.
async fn execute_value(
    client: &mut DaemonClient,
    command: Command,
) -> std::result::Result<Value, String> {
    let response = client
        .send::<Value>(command)
        .await
        .map_err(|error| format!("daemon request failed: {error:#}"))?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "unknown daemon error".into()));
    }
    Ok(response.data.unwrap_or(Value::Null))
}

/// Returns the job id a command targets, when it targets exactly one job.
/// Used to suggest near-matches when the daemon reports "job not found".
fn command_job_id(command: &Command) -> Option<String> {
    let id = match command {
        Command::Status { id }
        | Command::Stop { id, .. }
        | Command::Restart { id, .. }
        | Command::Tail(TailArgs { id, .. })
        | Command::Diff { id, .. }
        | Command::Wait { id, .. }
        | Command::Send { id, .. }
        | Command::Stats { id }
        | Command::Expect { id, .. }
        | Command::Screen { id, .. } => id,
        Command::Kill(args) => return args.id.clone(),
        _ => return None,
    };
    Some(id.clone())
}

/// Enriches "job not found" errors with up to three near-matching names or
/// id prefixes, so the caller can self-correct without a separate `list`
/// round trip. Any other error passes through untouched, and a failed
/// lookup never masks the original error.
async fn suggest_similar_jobs(
    message: &str,
    query_id: Option<&str>,
    client: &mut DaemonClient,
) -> String {
    let Some(query) = query_id.filter(|q| !q.is_empty()) else {
        return message.to_string();
    };
    if !message.contains("job not found") {
        return message.to_string();
    }
    let Ok(jobs) = execute_value(client, Command::List { workspace: None }).await else {
        return message.to_string();
    };
    let suggestions = similar_job_names(query, &jobs);
    if suggestions.is_empty() {
        message.to_string()
    } else {
        format!("{message}; did you mean: {}", suggestions.join(", "))
    }
}

/// Picks up to three job handles resembling `query`: exact-name containment
/// first, then id prefixes, then fuzzy name matches.
fn similar_job_names(query: &str, jobs: &Value) -> Vec<String> {
    let Some(jobs) = jobs.as_array() else {
        return Vec::new();
    };
    let query_lower = query.to_lowercase();
    let mut exact = Vec::new();
    let mut prefix = Vec::new();
    let mut fuzzy = Vec::new();
    for job in jobs {
        let name = job.get("name").and_then(Value::as_str).unwrap_or("");
        let id = job.get("id").and_then(Value::as_str).unwrap_or("");
        let handle = if name.is_empty() {
            format!("id {}", &id[..id.len().min(8)])
        } else {
            name.to_string()
        };
        if name.to_lowercase().contains(&query_lower) {
            exact.push(handle);
        } else if id.starts_with(query) {
            prefix.push(handle);
        } else if !name.is_empty() && query_lower.chars().all(|c| name.to_lowercase().contains(c)) {
            fuzzy.push(handle);
        }
    }
    exact
        .into_iter()
        .chain(prefix)
        .chain(fuzzy)
        .take(3)
        .collect()
}

/// Trims a `list` payload to the fields an agent usually needs: id, name,
/// state, pid, a shortened command, workspace. Drops `env`, `cwd`, and the
/// policy/config block. Full records are one `verbose: true` call away, and
/// ids are kept whole so they stay usable as handles in later calls.
fn trim_job_list(data: &Value) -> Value {
    let Some(jobs) = data.as_array() else {
        return data.clone();
    };
    Value::Array(
        jobs.iter()
            .map(|job| {
                json!({
                    "id": job.get("id"),
                    "name": job.get("name"),
                    "state": job.get("state"),
                    "pid": job.get("pid"),
                    "cmd": summarize_cmd(job.get("cmd")),
                    "workspace": job.get("workspace"),
                })
            })
            .collect(),
    )
}

/// Shortens a job's command vector to its first three words, with an
/// ellipsis marker when longer. Non-string commands pass through untouched.
fn summarize_cmd(cmd: Option<&Value>) -> Value {
    let Some(items) = cmd.and_then(Value::as_array) else {
        return cmd.cloned().unwrap_or(Value::Null);
    };
    let words: Vec<&str> = items
        .iter()
        .flat_map(|item| match item {
            Value::String(s) => s.split_whitespace().collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect();
    if words.len() > 3 {
        Value::String(format!("{} …", words[..3].join(" ")))
    } else {
        Value::String(words.join(" "))
    }
}

/// Enforces the per-tool output ceiling. Oversized text is cut at a UTF-8
/// boundary with a marker explaining how to re-query narrower, so the caller
/// knows the output is partial and what to do about it.
fn apply_text_ceiling(tool_name: &str, mut text: String) -> String {
    if text.len() <= MAX_TOOL_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_TOOL_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    let hint = match tool_name {
        "tail" | "diff" | "screen" => {
            "re-run with fewer `lines` or a filter (`level`, `stream`, `filter_regex`)"
        }
        "list" => "re-run with `verbose: false` (default) or a `workspace` filter",
        _ => "re-run with narrower arguments",
    };
    use std::fmt::Write;
    let _ = write!(
        text,
        "\n[truncated: response exceeded {MAX_TOOL_TEXT_BYTES} bytes; {hint}]"
    );
    text
}

/// Caps a caller-supplied line count at the per-tool maximum.
fn cap_lines(lines: usize) -> usize {
    lines.min(MAX_TOOL_LINES)
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
            "Start a background job and return its record. `cmd` is the command \
             vector (see schema for the optional fields). Set `wait: true` to \
             block until the job is ready in the same call instead of calling \
             `wait` separately. For interactive programs set `pty: true`, then \
             drive them with the `send`/`screen`/`expect` tools.",
            run_schema(),
        ),
        tool(
            "list",
            "List jobs (id, name, state, pid, command). Pass `verbose: true` \
             for the full record including `env` and `cwd`.",
            json!({
                "type": "object",
                "properties": {
                    "workspace": { "type": "string", "description": "Only list jobs in this workspace." },
                    "verbose": { "type": "boolean", "description": "Return full job records (default false)." }
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
            "Terminate a job: SIGTERM first, escalating to SIGKILL after 5s. \
             Fails on already-finished jobs (use `stop` to mark those). \
             Or kill every job in a workspace with `workspace` instead of `id`.",
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
            "Read the last `lines` (default 50, capped at 1000) of a job's log, \
             optionally filtered by `level` substring (e.g. \"error\"), `stream` \
             (stdout/stderr/pty) or `filter_regex`. With `digest: true` and no \
             `lines`, returns only the summary (total lines, error/warning \
             counts, last error).",
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
            "Drive an interactive job's stdin: start it with `pty: true`, then send input \
             here. End `data` with \"\\n\" for Enter. Control keys are raw bytes: Esc is \
             \"\\u001b\", Up/Down/Left/Right are \"\\u001b[A/B/C/D\", Tab is \"\\t\", \
             Ctrl+C is \"\\u0003\", Ctrl+D is \"\\u0004\". Read the result with `screen` \
             (or `expect` to block until a prompt appears).",
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
            "Block until `pattern` appears in the job's output (plain substring by default, \
             regex with `is_regex: true`), or `timeout_ms` elapses (default 60000). \
             Matches interactive prompts immediately — even mid-line ones like `>>> ` \
             that never reach the log file (returned with `line_number: null`).",
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
            "Read the last `lines` (default 20, capped at 1000) from a job's in-memory \
             output buffer without touching the log file. For interactive (PTY) jobs this \
             is how you see REPL output and prompts; pass `strip_ansi: true` to remove \
             color/cursor codes first.",
            json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Job ID or name." },
                    "lines": { "type": "integer", "description": "How many rows to return (default 20)." },
                    "strip_ansi": { "type": "boolean", "description": "Strip ANSI escape codes from content." }
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

/// The `run` tool's input schema: the `RunArgs` schema plus the MCP-only
/// `wait` flags. `RunArgs` deserialization ignores the extra properties, so
/// they are read separately in `execute()` after a successful spawn.
fn run_schema() -> Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(RunArgs))
        .unwrap_or_else(|_| json!({ "type": "object" }));
    // Drop schema metadata that guides nothing: the `$schema` URI and type
    // names. Property and definition `description`s stay — they document
    // restart policies and readiness shapes the caller must get right.
    if let Some(root) = schema.as_object_mut() {
        root.remove("$schema");
        root.remove("title");
        if let Some(definitions) = root.get_mut("definitions").and_then(Value::as_object_mut) {
            for definition in definitions.values_mut() {
                if let Some(definition) = definition.as_object_mut() {
                    definition.remove("title");
                }
            }
        }
    }
    if let Some(properties) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        properties.insert(
            "wait".to_string(),
            json!({ "type": "boolean", "description": "Wait until the job is ready before returning (default false)." }),
        );
        properties.insert(
            "wait_timeout_ms".to_string(),
            json!({ "type": "integer", "description": "Maximum wait after starting (default 60000)." }),
        );
    }
    schema
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
        "tail" => {
            let digest = get_bool(arguments, "digest", false);
            // A digest without an explicit `lines` returns only the summary.
            // Defaulting to 50 here as well would attach every log line to a
            // call that asked for a summary.
            let lines = match arguments.get("lines").filter(|value| !value.is_null()) {
                None if digest => 0,
                _ => cap_lines(get_usize(arguments, "lines", 50)?),
            };
            Ok(Command::Tail(TailArgs {
                id: require_str(arguments, "id")?,
                lines,
                digest,
                level: get_str(arguments, "level"),
                strip_ansi: get_bool(arguments, "strip_ansi", false),
                stream: get_str(arguments, "stream"),
                cursor: None,
                follow: false,
                filter_regex: get_str(arguments, "filter_regex"),
            }))
        }
        "diff" => Ok(Command::Diff {
            id: require_str(arguments, "id")?,
            lines: get_usize_opt(arguments, "lines")?.map(cap_lines),
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
            lines: cap_lines(get_usize(arguments, "lines", 20)?),
            strip_ansi: get_bool(arguments, "strip_ansi", false),
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
    fn tail_digest_without_lines_requests_summary_only() {
        // `digest: true` alone must not fetch log lines: the daemon returns
        // both whenever `lines > 0`, which made the "token-saving" digest
        // the most expensive call in the registry.
        let command = build_command("tail", &json!({ "id": "web", "digest": true })).unwrap();
        assert_eq!(
            command,
            Command::Tail(TailArgs {
                id: "web".into(),
                lines: 0,
                digest: true,
                level: None,
                strip_ansi: false,
                stream: None,
                cursor: None,
                follow: false,
                filter_regex: None,
            })
        );

        // An explicit `lines` still returns both, as documented.
        let command =
            build_command("tail", &json!({ "id": "web", "digest": true, "lines": 5 })).unwrap();
        assert!(matches!(command, Command::Tail(TailArgs { lines: 5, .. })));
    }

    #[test]
    fn log_reading_lines_are_capped() {
        let command = build_command("tail", &json!({ "id": "x", "lines": 10_000 })).unwrap();
        assert!(matches!(
            command,
            Command::Tail(TailArgs { lines: 1000, .. })
        ));

        let command = build_command("diff", &json!({ "id": "x", "lines": 10_000 })).unwrap();
        assert!(matches!(
            command,
            Command::Diff {
                lines: Some(1000),
                ..
            }
        ));

        let command = build_command("screen", &json!({ "id": "x", "lines": 10_000 })).unwrap();
        assert!(matches!(command, Command::Screen { lines: 1000, .. }));

        // Small values pass through untouched.
        let command = build_command("tail", &json!({ "id": "x", "lines": 5 })).unwrap();
        assert!(matches!(command, Command::Tail(TailArgs { lines: 5, .. })));
    }

    #[test]
    fn run_schema_accepts_wait_flags() {
        // The MCP-only flags must survive schema generation and be ignored
        // by `RunArgs` deserialization (read separately after a spawn).
        let schema = run_schema();
        assert!(schema["properties"].get("wait").is_some());
        assert!(schema["properties"].get("wait_timeout_ms").is_some());

        let command = build_command(
            "run",
            &json!({ "cmd": ["sleep", "30"], "wait": true, "wait_timeout_ms": 5000 }),
        )
        .unwrap();
        assert!(matches!(command, Command::Run(_)));
        // Same payload without the flags maps identically.
        let plain = build_command("run", &json!({ "cmd": ["sleep", "30"] })).unwrap();
        assert_eq!(command, plain);
    }

    #[test]
    fn list_trimming_drops_env_and_cwd_but_keeps_ids() {
        let jobs = json!([
            {
                "id": "abcdef01-2345-6789-abcd-ef0123456789",
                "name": "web",
                "state": "running",
                "pid": 123,
                "cmd": ["sh", "-c", "npm run dev --port 3000"],
                "cwd": "/home/user/project",
                "env": { "TERM": "xterm", "LANG": "en_US.UTF-8" },
                "workspace": null,
                "started_at": "2026-10-05T00:00:00Z"
            },
            { "id": "other", "name": null, "state": "exited", "cmd": ["true"] }
        ]);

        let trimmed = trim_job_list(&jobs);
        // Full ids survive: they are the handles for later calls.
        assert_eq!(trimmed[0]["id"], "abcdef01-2345-6789-abcd-ef0123456789");
        assert_eq!(trimmed[0]["name"], "web");
        assert_eq!(trimmed[0]["state"], "running");
        assert_eq!(trimmed[0]["pid"], 123);
        // Long commands shorten; short ones pass through.
        assert_eq!(trimmed[0]["cmd"], "sh -c npm …");
        assert_eq!(trimmed[1]["cmd"], "true");
        // Dropped fields stay dropped.
        for job in trimmed.as_array().unwrap() {
            assert!(job.get("env").is_none(), "{job}");
            assert!(job.get("cwd").is_none(), "{job}");
            assert!(job.get("started_at").is_none(), "{job}");
        }
        // Non-array payloads pass through for the caller to handle.
        assert_eq!(trim_job_list(&json!({"ok": true})), json!({"ok": true}));
    }

    #[test]
    fn truncation_marks_partial_output() {
        let big = "x".repeat(MAX_TOOL_TEXT_BYTES + 100);
        let cut = apply_text_ceiling("tail", big);
        assert!(cut.len() < MAX_TOOL_TEXT_BYTES + 200, "{}", cut.len());
        assert!(cut.contains("[truncated"), "{cut}");
        assert!(cut.contains("fewer `lines`"), "{cut}");

        let small = "short".to_string();
        assert_eq!(apply_text_ceiling("tail", small.clone()), small);

        // Multi-byte content is cut at a UTF-8 boundary, never mid-character.
        let wide = "é".repeat(MAX_TOOL_TEXT_BYTES);
        let cut = apply_text_ceiling("list", wide);
        assert!(cut.contains("[truncated"), "{cut}");
        assert!(std::str::from_utf8(cut.as_bytes()).is_ok());
    }

    #[test]
    fn similar_job_names_ranks_exact_matches_first() {
        let jobs = json!([
            { "id": "aaaabbbb-1", "name": "worker" },
            { "id": "web-1111", "name": "web" },
            { "id": "web-2222", "name": "website" },
            { "id": "zzzz9999", "name": "db" }
        ]);
        let suggestions = similar_job_names("web", &jobs);
        assert_eq!(suggestions, vec!["web", "website"]);
        assert!(similar_job_names("nomatchatall", &jobs).is_empty());
        assert!(similar_job_names("web", &json!({"not": "an array"})).is_empty());
    }

    #[test]
    fn kill_description_states_sigterm_first() {
        let tools = tool_registry();
        let kill = tools.iter().find(|tool| tool.name == "kill").unwrap();
        assert!(kill.description.contains("SIGTERM"), "{}", kill.description);
        assert!(
            !kill.description.contains("SIGKILL)"),
            "{}",
            kill.description
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
                strip_ansi: false,
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
