use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use bgrun_core::JobStore;
use bgrun_proto::{Command, KillArgs, Request, Response, TailArgs, WaitResult};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::runner;

/// Runs the Unix socket server, accepting connections until shutdown is signalled.
pub async fn run_server(
    socket_path: PathBuf,
    store: Arc<Mutex<JobStore>>,
    shutdown: CancellationToken,
) -> Result<()> {
    // Remove old socket file if it exists
    let _ = tokio::fs::remove_file(&socket_path).await;

    // Ensure parent directory exists
    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    info!("daemon listening on {}", socket_path.display());

    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => {
                info!("shutdown signal received, stopping server");
                break;
            }
            result = listener.accept() => {
                match result {
                    Ok((stream, _addr)) => {
                        let store = store.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, store).await {
                                error!("connection handler error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("accept error: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handles a single connection: reads NDJSON requests, dispatches, writes responses.
/// For Attach commands, it hijacks the connection for bidirectional byte streaming.
async fn handle_connection(
    stream: UnixStream,
    store: Arc<Mutex<JobStore>>,
) -> Result<()> {
    // Peek at the first request without consuming the stream.
    // Attach hijacks the stream, so we cannot split it upfront.
    let first_line = read_first_line(&stream).await;
    let first_line = match first_line {
        Some(l) => l,
        None => return Ok(()),
    };

    let request: Request = match serde_json::from_str(&first_line) {
        Ok(r) => r,
        Err(_) => return Ok(()),
    };

    // If it's an Attach or StreamLogs command, hijack the connection
    match request.command {
        Command::Attach { id } => return handle_attach(id, stream, store).await,
        Command::StreamLogs { id } => return handle_stream_logs(id, stream, store).await,
        _ => {}
    }

    // Normal path: split the stream and continue with NDJSON dispatching
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    // Dispatch the first request (already parsed before we split)
    let first_response = dispatch(
        Request {
            id: request.id.clone(),
            command: request.command.clone(),
        },
        store.clone(),
    )
    .await;
    let json = serde_json::to_string(&first_response)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;

    // Read subsequent requests
    loop {
        line.clear();
        match buf_reader.read_line(&mut line).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => {
                error!("read error: {}", e);
                break;
            }
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let response = dispatch(request, store.clone()).await;
        let json = serde_json::to_string(&response)?;
        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
    }

    Ok(())
}

/// Reads the first line from a UnixStream without consuming the stream.
/// Reads byte-by-byte up to a newline, with a 64KB limit.
async fn read_first_line(stream: &UnixStream) -> Option<String> {
    stream.readable().await.ok()?;
    let mut buf = vec![0u8; 1];
    let mut line = Vec::new();
    loop {
        match stream.try_read(&mut buf) {
            Ok(0) => return None,
            Ok(1) => {
                if buf[0] == b'\n' {
                    break;
                }
                line.push(buf[0]);
                if line.len() > 65536 {
                    return None;
                }
            }
            Ok(_) => continue,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                stream.readable().await.ok()?;
                continue;
            }
            Err(_) => return None,
        }
    }
    String::from_utf8(line).ok()
}

/// Handles an Attach command: hijacks the connection for bidirectional raw byte piping.
///
/// 1. Sends an initial NDJSON success response
/// 2. Forwards socket reads → PTY writer (stdin)
/// 3. Forwards broadcast PTY output → socket (stdout)
/// 4. Returns when either side disconnects or the job exits
async fn handle_attach(
    id: String,
    stream: UnixStream,
    store: Arc<Mutex<JobStore>>,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    // Resolve name to UUID and verify job is alive+PTY under single lock
    let job_id = {
        let store_ref = store.lock().await;
        let jid = store_ref.resolve_id(&id);
        match jid {
            Some(jid) if store_ref.get(&jid).is_some_and(|job| job.is_alive() && job.pty) => jid,
            _ => {
                let (_reader, mut writer) = stream.into_split();
                let err = Response::<()>::err("attach".into(), "job not found, not alive, or not a PTY job");
                let json = serde_json::to_string(&err).unwrap_or_default();
                let _ = writer.write_all(json.as_bytes()).await;
                let _ = writer.write_all(b"\n").await;
                return Ok(());
            }
        }
    };

    // Split the stream for bidirectional piping
    let (mut stream_read, mut stream_write) = stream.into_split();

    // Send initial success response
    let init = serde_json::json!({
        "id": "attach",
        "ok": true,
        "data": { "attached": true },
    });
    let json = serde_json::to_string(&init)?;
    stream_write.write_all(json.as_bytes()).await?;
    stream_write.write_all(b"\n").await?;

    // Get the PTY writer for stdin injection (use resolved job_id)
    let pty_writer = {
        let mut writers = runner::PTY_WRITERS.lock().await;
        writers.get_mut(&job_id).map(|w| w.clone())
    };

    let pty_writer = match pty_writer {
        Some(w) => w,
        None => return Ok(()),
    };

    // Subscribe to the broadcast channel for PTY output (use resolved job_id)
    let rx = {
        let broadcasts = runner::JOB_BROADCASTS.lock().await;
        broadcasts.get(&job_id).map(|tx| tx.subscribe())
    };

    let mut rx = match rx {
        Some(r) => r,
        None => return Ok(()),
    };

    // Shared signal to notify stdin forwarding when the job exits
    let exit_notify = Arc::new(tokio::sync::Notify::new());

    // Spawn task to forward broadcast PTY output → socket write half
    let write_half = Arc::new(tokio::sync::Mutex::new(stream_write));
    let write_half_clone = write_half.clone();

    let mut output_task: tokio::task::JoinHandle<()> = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(bytes) => {
                    let mut writer = write_half_clone.lock().await;
                    if writer.write_all(&bytes).await.is_err() {
                        break;
                    }
                    let _ = writer.flush().await;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    });

    // Spawn task to forward socket reads → PTY writer (stdin)
    let exit_notify_clone = exit_notify.clone();
    let mut stdin_task: tokio::task::JoinHandle<()> = tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            tokio::select! {
                result = stream_read.read(&mut buf) => {
                    match result {
                        Ok(0) => break,
                        Ok(n) => {
                            let data = buf[..n].to_vec();
                            let mut writer = pty_writer.lock().unwrap();
                            use std::io::Write;
                            if writer.write_all(&data).is_err() {
                                break;
                            }
                            let _ = writer.flush();
                        }
                        Err(_) => break,
                    }
                }
                _ = exit_notify_clone.notified() => break,
            }
        }
    });

    // Wait for either task to finish (borrow to avoid moves)
    tokio::select! {
        _ = &mut output_task => {
            // Job exited (broadcast closed); signal stdin task to stop
            exit_notify.notify_one();
            let _ = stdin_task.await;
        }
        _ = &mut stdin_task => {
            // Client disconnected; abort output task
            output_task.abort();
            let _ = output_task.await;
        }
    }

    Ok(())
}

/// Handles a StreamLogs command: hijacks the connection and streams LogLine
/// entries as NDJSON. First sends any lines already on disk, then subscribes
/// to the live broadcast channel.
async fn handle_stream_logs(
    id: String,
    stream: UnixStream,
    store: Arc<Mutex<JobStore>>,
) -> Result<()> {
    use bgrun_proto::LogLine;
    use tokio::io::AsyncWriteExt;

    // Resolve name to UUID and verify job exists under single lock
    let job_id = {
        let store_ref = store.lock().await;
        let jid = store_ref.resolve_id(&id);
        match jid {
            Some(jid) if store_ref.get(&jid).is_some() => jid,
            _ => {
                let (_reader, mut writer) = stream.into_split();
                let err = Response::<()>::err("stream".into(), "job not found");
                let json = serde_json::to_string(&err).unwrap_or_default();
                let _ = writer.write_all(json.as_bytes()).await;
                let _ = writer.write_all(b"\n").await;
                return Ok(());
            }
        }
    };

    // Read existing lines from disk
    let log_path = crate::state::job_dir(&job_id).join("stdout.log");
    let file_size = get_file_size(&log_path).await;
    let existing_lines = if file_size > 0 {
        let content = read_range(&log_path, 0, file_size).await.unwrap_or_default();
        content
            .lines()
            .filter_map(|raw| {
                let (timestamp, _stream, content) = bgrun_daemon::log_manager::parse_line(raw);
                Some(LogLine {
                    line_number: 0,
                    content,
                    timestamp,
                })
            })
            .collect::<Vec<LogLine>>()
    } else {
        Vec::new()
    };

    let (_, mut stream_write) = stream.into_split();

    // Send initial success response with existing lines count
    let init = serde_json::json!({
        "id": "stream",
        "ok": true,
        "data": {
            "existing": existing_lines.len(),
            "cursor": file_size,
        }
    });
    let json = serde_json::to_string(&init)?;
    stream_write.write_all(json.as_bytes()).await?;
    stream_write.write_all(b"\n").await?;

    // Send existing lines
    for line in &existing_lines {
        let json = serde_json::to_string(line)?;
        stream_write.write_all(json.as_bytes()).await?;
        stream_write.write_all(b"\n").await?;
    }

    // Subscribe to the broadcast channel for live LogLine stream
    let rx = {
        let broadcasts = runner::LOG_BROADCASTS.lock().await;
        broadcasts.get(&job_id).map(|tx| tx.subscribe())
    };

    let mut rx = match rx {
        Some(r) => r,
        None => return Ok(()),
    };

    // Spawn task to forward broadcast LogLines → socket
    let mut output_task: tokio::task::JoinHandle<()> = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(line) => {
                    let json = serde_json::to_string(&line).unwrap_or_default();
                    if stream_write.write_all(json.as_bytes()).await.is_err() {
                        break;
                    }
                    if stream_write.write_all(b"\n").await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    });

    // Also listen for job exit to close the stream
    let exit_notify = Arc::new(tokio::sync::Notify::new());
    let exit_notify_clone = exit_notify.clone();
    let store_clone = store.clone();
    let job_id_clone = job_id.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            let done = {
                let store_ref = store_clone.lock().await;
                store_ref.get(&job_id_clone).map_or(true, |j| !j.is_alive())
            };
            if done {
                exit_notify_clone.notify_one();
                break;
            }
        }
    });

    tokio::select! {
        _ = &mut output_task => {}
        _ = exit_notify.notified() => {
            output_task.abort();
            let _ = output_task.await;
        }
    }

    Ok(())
}

/// Dispatches a command to the appropriate handler.
async fn dispatch(
    request: Request,
    store: Arc<Mutex<JobStore>>,
) -> serde_json::Value {
    let req_id = request.id;
    let cmd_name = format!("{:?}", request.command);
    let audit_args = args_summary(&request.command);

    let resp: Response<serde_json::Value> = match request.command {
        Command::Run(args) => match runner::spawn_job(args, store).await {
            Ok(record) => match serde_json::to_value(record) {
                Ok(val) => Response::ok(req_id.clone(), val),
                Err(e) => Response::err(req_id.clone(), format!("serialization error: {}", e)),
            },
            Err(e) => Response::err(req_id.clone(), e.to_string()),
        },
        Command::Status { id } => {
            let store = store.lock().await;
            let job_id = store.resolve_id(&id);
            let job_id = match job_id {
                Some(jid) => jid,
                None => return serde_json::to_value(Response::<()>::err(req_id.clone(), "job not found")).unwrap_or_default(),
            };
            match store.get(&job_id) {
                Some(job) => {
                    let status = bgrun_proto::JobStatus {
                        state: job.state.clone(),
                        exit_code: job.exit_code,
                        ready_at: job.ready_at.map(|t| t.to_rfc3339()),
                        restart_count: job.restart_count,
                        last_diff_cursor: job.last_diff_cursor,
                        consecutive_failures: job.consecutive_failures,
                    };
                    match serde_json::to_value(status) {
                        Ok(val) => Response::ok(req_id.clone(), val),
                        Err(e) => {
                            Response::err(req_id.clone(), format!("serialization error: {}", e))
                        }
                    }
                }
                None => Response::err(req_id.clone(), "job not found"),
            }
        }
        Command::List { workspace } => {
            let store = store.lock().await;
            let jobs = store.list_workspace(workspace.as_deref());
            let records: Vec<bgrun_proto::JobRecord> = jobs.iter().map(|j| j.to_record()).collect();
            match serde_json::to_value(records) {
                Ok(val) => Response::ok(req_id.clone(), val),
                Err(e) => Response::err(req_id.clone(), format!("serialization error: {}", e)),
            }
        }
        Command::Kill(KillArgs { id, workspace }) => match id {
            Some(id) => match runner::kill_job(&id, store).await {
                Ok(()) => Response::ok(req_id.clone(), serde_json::json!({"killed": [id]})),
                Err(e) => Response::err(req_id.clone(), e.to_string()),
            },
            None => match workspace {
                Some(workspace) => {
                    let ids: Vec<String> = {
                        let store = store.lock().await;
                        store
                            .list_workspace(Some(&workspace))
                            .into_iter()
                            .filter(|job| job.is_alive())
                            .map(|job| job.id.clone())
                            .collect()
                    };

                    let mut killed = Vec::new();
                    let mut errors = Vec::new();
                    for id in ids {
                        match runner::kill_job(&id, store.clone()).await {
                            Ok(()) => killed.push(id),
                            Err(err) => errors.push(format!("{id}: {err}")),
                        }
                    }

                    if errors.is_empty() {
                        Response::ok(req_id.clone(), serde_json::json!({ "killed": killed }))
                    } else {
                        Response::err(req_id.clone(), errors.join("; "))
                    }
                }
                None => Response::err(req_id.clone(), "id or workspace is required"),
            },
        },
        Command::Wait { id, timeout_ms } => {
            let start = tokio::time::Instant::now();
            let timeout = std::time::Duration::from_millis(timeout_ms);
            let mut ready = false;
            let mut exit_code = None;
            let mut state = None;

            // Resolve name to UUID inside the loop so that if the name is
            // reassigned to a new UUID (e.g. after a restart) we pick it up.
            loop {
                if start.elapsed() >= timeout {
                    break;
                }
                {
                    let store_ref = store.lock().await;
                    let resolved_id = store_ref.resolve_id(&id);
                    match resolved_id.and_then(|jid| store_ref.get(&jid)) {
                        Some(job) if job.state == bgrun_proto::JobState::Ready
                            || job.ready_at.is_some() =>
                        {
                            ready = true;
                            exit_code = job.exit_code;
                            state = Some(job.state.to_string());
                            break;
                        }
                        Some(job)
                            if matches!(
                                job.state,
                                bgrun_proto::JobState::Exited
                                    | bgrun_proto::JobState::Crashed
                                    | bgrun_proto::JobState::Killed
                            ) =>
                        {
                            exit_code = job.exit_code;
                            state = Some(job.state.to_string());
                            break;
                        }
                        Some(_) => {}
                        None => {
                            let err = Response::<()>::err(req_id, "job not found".to_string());
                            return serde_json::to_value(err).unwrap_or_default();
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }

            let elapsed_ms = start.elapsed().as_millis() as u64;
            let result = WaitResult {
                ready,
                elapsed_ms,
                exit_code,
                state,
            };
            match serde_json::to_value(result) {
                Ok(val) => Response::ok(req_id.clone(), val),
                Err(e) => Response::err(req_id.clone(), format!("serialization error: {}", e)),
            }
        }
        Command::Tail(TailArgs {
            id,
            lines,
            digest,
            level,
            strip_ansi,
            stream,
            cursor: cursor_opt,
            follow,
            filter_regex,
        }) => {
            // Resolve name to UUID under lock and verify job exists
            let job_id = {
                let store_ref = store.lock().await;
                let jid = store_ref.resolve_id(&id);
                match jid {
                    Some(ref jid) if store_ref.get(jid).is_some() => jid.clone(),
                    _ => return serde_json::to_value(Response::<()>::err(req_id.clone(), "job not found")).unwrap_or_default(),
                }
            };
            let stream_deref = stream.as_deref();
            let level_deref = level.as_deref();
            let regex_ref = filter_regex.as_ref().and_then(|p| regex::Regex::new(p).ok());
            let regex_deref = regex_ref.as_ref();
            if digest {
                match bgrun_daemon::log_manager::tail_digest(&job_id).await {
                    Ok(digest) => {
                        // If --lines is also specified, include the last N lines
                        if lines > 0 {
                            match bgrun_daemon::log_manager::tail_lines(&job_id, lines, stream_deref, level_deref, regex_deref).await {
                                Ok(mut log_lines) => {
                                    if strip_ansi {
                                        for line in &mut log_lines {
                                            let clean = strip_ansi_escapes::strip(line.content.as_bytes());
                                            line.content = String::from_utf8_lossy(&clean).into_owned();
                                        }
                                    }
                                    let combined = serde_json::json!({
                                        "digest": digest,
                                        "lines": log_lines,
                                    });
                                    match serde_json::to_value(combined) {
                                        Ok(val) => Response::ok(req_id.clone(), val),
                                        Err(e) => Response::err(req_id.clone(), format!("serialization error: {}", e)),
                                    }
                                }
                                Err(_) => match serde_json::to_value(digest) {
                                    Ok(val) => Response::ok(req_id.clone(), val),
                                    Err(e) => Response::err(req_id.clone(), format!("serialization error: {}", e)),
                                },
                            }
                        } else {
                            match serde_json::to_value(digest) {
                                Ok(val) => Response::ok(req_id.clone(), val),
                                Err(e) => Response::err(req_id.clone(), format!("serialization error: {}", e)),
                            }
                        }
                    }
                    Err(e) => Response::err(req_id.clone(), e.to_string()),
                }
            } else if let Some(cursor) = cursor_opt {
                // Cursor-based read for follow mode
                match bgrun_daemon::log_manager::diff_since(&job_id, cursor, stream_deref, level_deref, regex_deref).await {
                    Ok((mut log_lines, new_cursor)) => {
                        // Strip ANSI escape codes if requested
                        if strip_ansi {
                            for line in &mut log_lines {
                                let clean = strip_ansi_escapes::strip(line.content.as_bytes());
                                line.content = String::from_utf8_lossy(&clean).into_owned();
                            }
                        }
                        let lines_json = serde_json::json!({
                            "lines": log_lines,
                            "cursor": new_cursor,
                        });
                        match serde_json::to_value(lines_json) {
                            Ok(val) => Response::ok(req_id.clone(), val),
                            Err(e) => {
                                Response::err(req_id.clone(), format!("serialization error: {}", e))
                            }
                        }
                    }
                    Err(e) => Response::err(req_id.clone(), e.to_string()),
                }
            } else {
                match bgrun_daemon::log_manager::tail_lines(&job_id, lines, stream_deref, level_deref, regex_deref).await {
                    Ok(mut log_lines) => {
                        // Strip ANSI escape codes if requested
                        if strip_ansi {
                            for line in &mut log_lines {
                                let clean = strip_ansi_escapes::strip(line.content.as_bytes());
                                line.content = String::from_utf8_lossy(&clean).into_owned();
                            }
                        }
                        // Get file size for cursor if follow mode
                        let log_path = bgrun_daemon::state::job_dir(&job_id).join("stdout.log");
                        let file_size = get_file_size(&log_path).await;
                        let mut result = serde_json::json!({
                            "lines": log_lines,
                        });
                        if follow {
                            result["cursor"] = serde_json::json!(file_size);
                        }
                        match serde_json::to_value(result) {
                            Ok(val) => Response::ok(req_id.clone(), val),
                            Err(e) => {
                                Response::err(req_id.clone(), format!("serialization error: {}", e))
                            }
                        }
                    }
                    Err(e) => Response::err(req_id.clone(), e.to_string()),
                }
            }
        }
        Command::Diff {
            id,
            lines,
            strip_ansi,
            stream,
            filter_regex,
        } => {
            // Resolve name to UUID under lock and read cursor atomically
            let (job_id, cursor) = {
                let store_ref = store.lock().await;
                let jid = store_ref.resolve_id(&id);
                match jid {
                    Some(jid) => (jid.clone(), store_ref.get(&jid).map_or(0, |job| job.last_diff_cursor)),
                    None => return serde_json::to_value(Response::<()>::err(req_id.clone(), "job not found")).unwrap_or_default(),
                }
            };
            let regex_ref = filter_regex.as_ref().and_then(|p| regex::Regex::new(p).ok());
            match bgrun_daemon::log_manager::diff_since(&job_id, cursor, stream.as_deref(), None, regex_ref.as_ref()).await {
                Ok((mut log_lines, new_cursor)) => {
                    // Truncate to last N lines if requested; only advance
                    // cursor by the number of lines actually returned so
                    // subsequent calls resume from where we left off.
                    let actual_new = if let Some(max_lines) = lines {
                        if log_lines.len() > max_lines {
                            let keep = log_lines.split_off(log_lines.len() - max_lines);
                            let count = keep.len() as u64;
                            log_lines = keep;
                            cursor + count
                        } else {
                            new_cursor
                        }
                    } else {
                        new_cursor
                    };

                    // Update cursor in store and persist
                    {
                        let mut store_ref = store.lock().await;
                        if let Some(job) = store_ref.get_mut(&job_id) {
                            job.last_diff_cursor = actual_new;
                        }
                    }
                    // Persist updated status
                    {
                        let store_ref = store.lock().await;
                        if let Some(job) = store_ref.get(&job_id) {
                            let _ = bgrun_daemon::state::write_status(job).await;
                        }
                    }
                    // Strip ANSI escape codes if requested
                    if strip_ansi {
                        for line in &mut log_lines {
                            let clean = strip_ansi_escapes::strip(line.content.as_bytes());
                            line.content = String::from_utf8_lossy(&clean).into_owned();
                        }
                    }
                    let result = serde_json::json!({
                        "lines": log_lines,
                        "cursor": actual_new,
                    });
                    match serde_json::to_value(result) {
                        Ok(val) => Response::ok(req_id.clone(), val),
                        Err(e) => Response::err(req_id.clone(), format!("serialization error: {}", e)),
                    }
                }
                Err(e) => Response::err(req_id.clone(), e.to_string()),
            }
        }
        Command::Send { id, data } => match runner::send_stdin(&id, &data, store.clone()).await {
            Ok(()) => Response::ok(req_id.clone(), serde_json::json!({ "ok": true })),
            Err(e) => Response::err(req_id.clone(), e.to_string()),
        },
        Command::Stats { id } => {
            match runner::get_stats(&id, store.clone()).await {
                Ok(stats) => match serde_json::to_value(stats) {
                    Ok(val) => Response::ok(req_id.clone(), val),
                    Err(e) => Response::err(req_id.clone(), format!("serialization error: {}", e)),
                },
                Err(e) => Response::err(req_id.clone(), e.to_string()),
            }
        }
        Command::Expect {
            id,
            pattern,
            is_regex,
            timeout_ms,
        } => {
            let start = tokio::time::Instant::now();
            let timeout = std::time::Duration::from_millis(timeout_ms);

            // Resolve name to UUID and verify job is alive under single lock
            let job_id = {
                let store_ref = store.lock().await;
                let jid = store_ref.resolve_id(&id);
                match jid {
                    Some(jid) => match store_ref.get(&jid) {
                        Some(job) if job.is_alive() => jid,
                        Some(_) => {
                            let err = Response::<()>::err(req_id.clone(), "job is not alive");
                            return serde_json::to_value(err).unwrap_or_default();
                        }
                        None => {
                            let err = Response::<()>::err(req_id.clone(), "job not found");
                            return serde_json::to_value(err).unwrap_or_default();
                        }
                    },
                    None => {
                        let err = Response::<()>::err(req_id.clone(), "job not found");
                        return serde_json::to_value(err).unwrap_or_default();
                    }
                }
            };

            let log_path = bgrun_daemon::state::job_dir(&job_id).join("stdout.log");
            let mut cursor = get_file_size(&log_path).await;
            let mut line_offset = count_lines_up_to(&log_path, cursor).await;

            loop {
                if start.elapsed() >= timeout {
                    let result = serde_json::json!({
                        "matched": false,
                        "line_number": null,
                        "content": null,
                    });
                    break Response::ok(req_id.clone(), result);
                }

                // Check if job is still alive
                {
                    let store_ref = store.lock().await;
                    match store_ref.get(&job_id) {
                        Some(job) if job.is_alive() => {}
                        _ => {
                            let err = Response::<()>::err(req_id.clone(), "job exited before pattern was matched");
                            return serde_json::to_value(err).unwrap_or_default();
                        }
                    }
                }

                // Check for new content in the log
                let file_size = get_file_size(&log_path).await;
                if file_size > cursor {
                    let new_content = read_range(&log_path, cursor, file_size).await;
                    cursor = file_size;

                    if let Some(content) = new_content {
                        let mut matched_line: Option<(u64, String)> = None;

                        for (i, line) in content.lines().enumerate() {
                            let found = if is_regex {
                                regex::Regex::new(&pattern)
                                    .ok()
                                    .is_some_and(|re| re.is_match(line))
                            } else {
                                line.contains(&pattern)
                            };

                            if found {
                                let line_number = line_offset + i as u64 + 1;
                                matched_line = Some((line_number, line.to_string()));
                                break;
                            }
                        }

                        if let Some((line_number, line_content)) = matched_line {
                            let result = serde_json::json!({
                                "matched": true,
                                "line_number": line_number,
                                "content": line_content,
                            });
                            break Response::ok(req_id.clone(), result);
                        }

                        line_offset += content.lines().count() as u64;
                    }
                }

                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
        Command::RunGroup { jobs } => {
            let mut records = Vec::new();
            let mut errors = Vec::new();
            for args in jobs {
                let label = args.name.clone().unwrap_or_else(|| "unnamed".into());
                match runner::spawn_job(args, store.clone()).await {
                    Ok(record) => records.push(record),
                    Err(e) => errors.push(format!("{}: {}", label, e)),
                }
            }
            if errors.is_empty() {
                match serde_json::to_value(records) {
                    Ok(val) => Response::ok(req_id.clone(), val),
                    Err(e) => Response::err(req_id.clone(), format!("serialization error: {}", e)),
                }
            } else {
                Response::err(req_id.clone(), errors.join("; "))
            }
        }
        Command::ResizePty { id, cols, rows } => {
            // Resolve name to UUID under lock
            let job_id = {
                let store_ref = store.lock().await;
                let jid = store_ref.resolve_id(&id);
                match jid {
                    Some(jid) => jid,
                    None => return serde_json::to_value(Response::<()>::err(req_id.clone(), "job not found")).unwrap_or_default(),
                }
            };
            let mut masters = runner::PTY_PAIRS.lock().await;
            match masters.get_mut(&job_id) {
                Some(master) => {
                    if let Err(e) = master.resize(portable_pty::PtySize {
                        cols,
                        rows,
                        pixel_width: 0,
                        pixel_height: 0,
                    }) {
                        Response::err(req_id.clone(), format!("resize failed: {}", e))
                    } else {
                        Response::ok(req_id.clone(), serde_json::json!({"resized": true}))
                    }
                }
                None => Response::err(req_id.clone(), "no PTY master for job"),
            }
        }
        Command::Clean { workspace } => {
            let to_remove: Vec<String> = {
                let store_ref = store.lock().await;
                store_ref
                    .list_workspace(workspace.as_deref())
                    .into_iter()
                    .filter(|j| !j.is_alive())
                    .map(|j| j.id.clone())
                    .collect()
            };
            let count = to_remove.len();
            for id in &to_remove {
                store.lock().await.remove(id);
                let _ = tokio::fs::remove_dir_all(bgrun_daemon::state::job_dir(id)).await;
            }
            info!(count = %count, "cleaned terminal-state jobs");
            Response::ok(req_id.clone(), serde_json::json!({"removed": count}))
        }
        Command::Attach { .. } => {
            // Attach is handled upstream in handle_connection before dispatch.
            // This arm exists for exhaustiveness but should never be reached.
            Response::err(req_id.clone(), "attach not supported via dispatch")
        }
        Command::StreamLogs { .. } => {
            // StreamLogs is handled upstream in handle_connection before dispatch.
            Response::err(req_id.clone(), "stream logs not supported via dispatch")
        }
        Command::Screen { id, lines } => {
            // Resolve name to UUID under lock
            let actual_id = {
                let store_ref = store.lock().await;
                let jid = store_ref.resolve_id(&id);
                match jid {
                    Some(jid) => jid,
                    None => return serde_json::to_value(Response::<()>::err(req_id.clone(), "job not found")).unwrap_or_default(),
                }
            };
            let buffers = runner::SCREEN_BUFFERS.lock().await;
            let content = buffers.get(&actual_id).map(|buf| {
                // Return last N lines from the ring buffer
                let bytes: Vec<u8> = buf.iter().copied().collect();
                let text = String::from_utf8_lossy(&bytes);
                let all_lines: Vec<&str> = text.lines().collect();
                let count = lines.min(all_lines.len());
                let tail: Vec<String> = all_lines[all_lines.len().saturating_sub(count)..]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                tail
            });
            drop(buffers);
            let lines = content.unwrap_or_default();
            match serde_json::to_value(lines) {
                Ok(val) => Response::ok(req_id.clone(), val),
                Err(e) => Response::err(req_id.clone(), format!("serialization error: {e}")),
            }
        }
    };

    // Record audit entry with a concise args summary
    let ok = resp.ok;
    let err_msg = resp.error.as_deref();
    bgrun_daemon::audit::record(&cmd_name, &audit_args, ok, err_msg).await;

    match serde_json::to_value(&resp) {
        Ok(val) => val,
        Err(e) => serde_json::to_value(Response::<()>::err(
            req_id,
            format!("serialization error: {}", e),
        ))
        .unwrap_or_default(),
    }
}

/// Returns the size of a file, or 0 if it doesn't exist.
async fn get_file_size(path: &std::path::Path) -> u64 {
    tokio::fs::metadata(path)
        .await
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Reads bytes from `start` to `end` in a file.
async fn read_range(path: &std::path::Path, start: u64, end: u64) -> Option<String> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .await
        .ok()?;
    file.seek(std::io::SeekFrom::Start(start)).await.ok()?;
    let len = (end - start) as usize;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).await.ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Builds a concise argument summary string for audit logging.
fn args_summary(cmd: &Command) -> String {
    match cmd {
        Command::Run(args) => {
            let cmd_str = args.cmd.join(" ");
            if let Some(ref name) = args.name {
                format!("name={} cmd=\"{}\"", name, cmd_str)
            } else {
                format!("cmd=\"{}\"", cmd_str)
            }
        }
        Command::Kill(KillArgs { id, workspace }) => match (id, workspace) {
            (Some(id), _) => format!("id={}", id),
            (_, Some(ws)) => format!("workspace={}", ws),
            _ => "".into(),
        },
        Command::Tail(TailArgs { id, lines, filter_regex, .. }) => {
            let mut s = format!("id={} lines={}", id, lines);
            if filter_regex.is_some() {
                s.push_str(" filter=regex");
            }
            s
        }
        Command::Status { id } => format!("id={}", id),
        Command::List { workspace } => workspace.clone().unwrap_or_else(|| "*".into()),
        Command::Wait { id, .. } => format!("id={}", id),
        Command::Diff { id, filter_regex, .. } => {
            let mut s = format!("id={}", id);
            if filter_regex.is_some() {
                s.push_str(" filter=regex");
            }
            s
        }
        Command::Send { id, .. } => format!("id={}", id),
        Command::Stats { id } => format!("id={}", id),
        Command::Expect { id, pattern, .. } => format!("id={} pattern=\"{}\"", id, pattern),
        Command::Attach { id } => format!("id={}", id),
        Command::ResizePty { id, cols, rows } => format!("id={} {}x{}", id, cols, rows),
        Command::Clean { workspace } => workspace.clone().unwrap_or_else(|| "*".into()),
        Command::RunGroup { jobs } => format!("{} jobs", jobs.len()),
        Command::StreamLogs { id } => format!("id={}", id),
        Command::Screen { id, lines } => format!("id={} lines={}", id, lines),
    }
}

/// Counts the number of newlines in a file up to the given byte offset.
async fn count_lines_up_to(path: &std::path::Path, offset: u64) -> u64 {
    use tokio::io::AsyncReadExt;

    if offset == 0 {
        return 0;
    }
    let mut file = match tokio::fs::OpenOptions::new()
        .read(true)
        .open(path)
        .await
    {
        Ok(f) => f,
        Err(_) => return 0,
    };
    if file.seek(std::io::SeekFrom::Start(0)).await.is_err() {
        return 0;
    }
    let to_read = offset.min(10 * 1024 * 1024) as usize; // cap at 10MB
    let mut buf = vec![0u8; to_read];
    let n = match file.read(&mut buf).await {
        Ok(n) => n,
        Err(_) => return 0,
    };
    buf[..n].iter().filter(|&&b| b == b'\n').count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use bgrun_core::JobStore;

    #[tokio::test]
    async fn status_for_missing_job_returns_error() {
        let request = Request {
            id: "req-1".into(),
            command: Command::Status {
                id: "missing".into(),
            },
        };
        let response = dispatch(
            request,
            Arc::new(Mutex::new(JobStore::new())),
        )
        .await;
        assert_eq!(response["ok"], false);
    }
}
