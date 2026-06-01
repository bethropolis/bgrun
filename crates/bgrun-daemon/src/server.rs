use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use bgrun_core::JobStore;
use bgrun_proto::{Command, KillArgs, Request, Response, TailArgs, WaitResult};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;
use tracing::{error, info};

use crate::runner;

type SharedSystem = Arc<Mutex<sysinfo::System>>;

/// Runs the Unix socket server, accepting connections forever.
pub async fn run_server(
    socket_path: PathBuf,
    store: Arc<Mutex<JobStore>>,
    sysinfo_system: SharedSystem,
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
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let store = store.clone();
                let sysinfo_system = sysinfo_system.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, store, sysinfo_system).await {
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

/// Handles a single connection: reads NDJSON requests, dispatches, writes responses.
async fn handle_connection(
    stream: UnixStream,
    store: Arc<Mutex<JobStore>>,
    sysinfo_system: SharedSystem,
) -> Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    // Read one request per connection (simplified for Phase 1)
    loop {
        line.clear();
        match buf_reader.read_line(&mut line).await {
            Ok(0) => break, // EOF
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
            Err(e) => {
                let err_resp =
                    Response::<()>::err("unknown".into(), format!("invalid request: {}", e));
                let _ = writer
                    .write_all(serde_json::to_string(&err_resp)?.as_bytes())
                    .await;
                let _ = writer.write_all(b"\n").await;
                continue;
            }
        };

        let response = dispatch(request, store.clone(), sysinfo_system.clone()).await;
        let json = serde_json::to_string(&response)?;
        writer.write_all(json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
    }

    Ok(())
}

/// Dispatches a command to the appropriate handler.
async fn dispatch(
    request: Request,
    store: Arc<Mutex<JobStore>>,
    sysinfo_system: SharedSystem,
) -> serde_json::Value {
    let req_id = request.id;
    let cmd_name = format!("{:?}", request.command);

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
            match store.get(&id) {
                Some(job) => {
                    let status = bgrun_proto::JobStatus {
                        state: job.state.clone(),
                        exit_code: job.exit_code,
                        ready_at: job.ready_at.map(|t| t.to_rfc3339()),
                        restart_count: job.restart_count,
                        last_diff_cursor: job.last_diff_cursor,
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

            loop {
                if start.elapsed() >= timeout {
                    break;
                }
                {
                    let store_ref = store.lock().await;
                    match store_ref.get(&id) {
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
        }) => {
            if digest {
                match bgrun_daemon::log_manager::tail_digest(&id).await {
                    Ok(digest) => match serde_json::to_value(digest) {
                        Ok(val) => Response::ok(req_id.clone(), val),
                        Err(e) => {
                            Response::err(req_id.clone(), format!("serialization error: {}", e))
                        }
                    },
                    Err(e) => Response::err(req_id.clone(), e.to_string()),
                }
            } else {
                match bgrun_daemon::log_manager::tail_lines(&id, lines).await {
                    Ok(mut log_lines) => {
                        // Filter by level if specified
                        if let Some(ref lvl) = level {
                            let lvl_lower = lvl.to_lowercase();
                            log_lines
                                .retain(|line| line.content.to_lowercase().contains(&lvl_lower));
                        }
                        let lines_json = serde_json::json!({
                            "lines": log_lines,
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
            }
        }
        Command::Diff { id, lines } => {
            // Read current cursor from store
            let cursor = {
                let store_ref = store.lock().await;
                store_ref.get(&id).map_or(0, |job| job.last_diff_cursor)
            };
            match bgrun_daemon::log_manager::diff_since(&id, cursor).await {
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
                        if let Some(job) = store_ref.get_mut(&id) {
                            job.last_diff_cursor = actual_new;
                        }
                    }
                    // Persist updated status
                    {
                        let store_ref = store.lock().await;
                        if let Some(job) = store_ref.get(&id) {
                            let _ = bgrun_daemon::state::write_status(job).await;
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
            match runner::get_stats(&id, store.clone(), sysinfo_system).await {
                Ok(stats) => match serde_json::to_value(stats) {
                    Ok(val) => Response::ok(req_id.clone(), val),
                    Err(e) => Response::err(req_id.clone(), format!("serialization error: {}", e)),
                },
                Err(e) => Response::err(req_id.clone(), e.to_string()),
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
    };

    // Record audit entry
    let ok = resp.ok;
    let err_msg = resp.error.as_deref();
    bgrun_daemon::audit::record(&cmd_name, "", ok, err_msg).await;

    match serde_json::to_value(&resp) {
        Ok(val) => val,
        Err(e) => serde_json::to_value(Response::<()>::err(
            req_id,
            format!("serialization error: {}", e),
        ))
        .unwrap_or_default(),
    }
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
            Arc::new(Mutex::new(sysinfo::System::new())),
        )
        .await;
        assert_eq!(response["ok"], false);
    }
}
