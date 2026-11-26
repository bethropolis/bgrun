//! Async bridge between the TUI and the bgrun daemon.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Result, bail};
use bgrun_proto::{Command, JobRecord, Request, ResourceStats, TailArgs};
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval};
use tokio_util::sync::CancellationToken;
use tui_term::vt100;

use super::state::{App, PollCtl, is_alive};

#[derive(Debug)]
pub enum DaemonEvent {
    JobsUpdated(Vec<JobRecord>),
    StatsUpdated { id: String, stats: ResourceStats },
    PtyBytes { id: String, bytes: Vec<u8> },
    PtyClosed { id: String },
    DigestLoaded(bgrun_proto::LogDigest),
    LogsLoaded(Vec<String>),
    DaemonIdle,
    Toast(String),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Pty,
    Logs,
}

/// A cancellable stream feeding the viewport for the selected job.
#[derive(Debug)]
pub struct ActiveStream {
    pub id: String,
    pub kind: StreamKind,
    pub cancel: CancellationToken,
    pub stdin_tx: Option<mpsc::UnboundedSender<Vec<u8>>>,
    pub read_task: JoinHandle<()>,
    pub write_task: Option<JoinHandle<()>>,
    pub last_size: (u16, u16),
}

impl ActiveStream {
    pub fn send_bytes(&self, bytes: Vec<u8>) {
        if let Some(tx) = &self.stdin_tx {
            let _ = tx.send(bytes);
        }
    }

    pub fn cancel(self) {
        self.cancel.cancel();
        self.read_task.abort();
        if let Some(task) = self.write_task {
            task.abort();
        }
    }
}

/// Sends one command and returns its `data` payload (if any).
pub async fn rpc<T: DeserializeOwned>(socket_path: &Path, command: Command) -> Result<Option<T>> {
    let mut client = crate::client::DaemonClient::connect(socket_path).await?;
    let response = client.send::<T>(command).await?;
    if !response.ok {
        bail!(
            "{}",
            response.error.unwrap_or_else(|| "daemon error".into())
        );
    }
    Ok(response.data)
}

/// Sends one user-initiated command, starting the daemon first if it idled out.
pub async fn rpc_autostart<T: DeserializeOwned>(
    socket_path: &Path,
    command: Command,
) -> Result<Option<T>> {
    crate::autostart::ensure_daemon_running(socket_path).await?;
    rpc(socket_path, command).await
}

/// Fetches jobs in the same order as `bgrun list`.
pub async fn fetch_jobs(socket_path: &Path, workspace: Option<String>) -> Result<Vec<JobRecord>> {
    let mut jobs = rpc::<Vec<JobRecord>>(socket_path, Command::List { workspace })
        .await?
        .unwrap_or_default();
    jobs.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    Ok(jobs)
}

/// Polls the job list at the configured interval; also refreshes stats for the
/// selection. A missing daemon is expected after idle shutdown and never causes
/// this passive task to autostart it.
pub fn spawn_poller(
    socket_path: PathBuf,
    tx: mpsc::UnboundedSender<DaemonEvent>,
    ctl: Arc<Mutex<PollCtl>>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let initial_ms = ctl.lock().unwrap_or_else(PoisonError::into_inner).poll_ms;
        let mut ticker = interval(Duration::from_millis(initial_ms));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let (workspace, selected, poll_ms) = {
                let ctl = ctl.lock().unwrap_or_else(PoisonError::into_inner);
                (ctl.workspace.clone(), ctl.selected.clone(), ctl.poll_ms)
            };
            let poll_duration = Duration::from_millis(poll_ms);
            if ticker.period() != poll_duration {
                ticker = interval(poll_duration);
                ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            }
            if let Ok(jobs) = fetch_jobs(&socket_path, workspace).await {
                let _ = tx.send(DaemonEvent::JobsUpdated(jobs));
            } else {
                let _ = tx.send(DaemonEvent::DaemonIdle);
                continue;
            }
            if let Some(id) = selected
                && let Ok(Some(stats)) =
                    rpc::<ResourceStats>(&socket_path, Command::Stats { id: id.clone() }).await
            {
                let _ = tx.send(DaemonEvent::StatsUpdated { id, stats });
            }
        }
    })
}

/// Replaces the viewport stream when the selected job changes.
pub async fn sync_active_stream(
    app: &mut App,
    active: &mut Option<ActiveStream>,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) {
    let Some(job) = app.selected_job() else {
        if let Some(stream) = active.take() {
            stream.cancel();
        }
        return;
    };

    let kind = if job.pty && is_alive(&job.state) {
        StreamKind::Pty
    } else {
        StreamKind::Logs
    };
    if let Some(stream) = active.as_ref()
        && stream.id == job.id
        && stream.kind == kind
    {
        return;
    }
    if let Some(stream) = active.take() {
        stream.cancel();
    }

    let id = job.id.clone();
    let alive = is_alive(&job.state);
    let size = app.stage_inner_size;
    let (cols, rows) = (size.0.max(2), size.1.max(2));
    app.parser = vt100::Parser::new(rows, cols, 2000);
    app.scroll_offset = 0;

    let result = match kind {
        StreamKind::Pty => spawn_pty_stream(socket_path, id.clone(), tx.clone()).await,
        StreamKind::Logs => {
            spawn_log_stream(socket_path, id.clone(), alive, tx.clone(), size).await
        }
    };
    match result {
        Ok(stream) => *active = Some(stream),
        Err(err) => {
            let _ = tx.send(DaemonEvent::Error(format!("attach {id}: {err:#}")));
        }
    }
}

/// Opens a raw PTY attach stream and forwards bytes both ways.
async fn spawn_pty_stream(
    socket_path: &Path,
    id: String,
    tx: mpsc::UnboundedSender<DaemonEvent>,
) -> Result<ActiveStream> {
    let stream = UnixStream::connect(socket_path).await?;
    let request = Request {
        id: uuid::Uuid::new_v4().to_string(),
        command: Command::Attach {
            id: id.clone(),
            replay: true,
        },
    };
    let mut payload = serde_json::to_string(&request)?;
    payload.push('\n');

    let (mut reader, mut writer) = tokio::io::split(stream);
    writer.write_all(payload.as_bytes()).await?;
    writer.flush().await?;

    // Read the ack byte-by-byte: a BufReader would swallow replay bytes past
    // the newline into its internal buffer.
    let mut ack = Vec::new();
    let mut byte = [0u8; 1];
    while reader.read(&mut byte).await? != 0 {
        ack.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }
    let ack: serde_json::Value = serde_json::from_slice(&ack)?;
    if !ack["ok"].as_bool().unwrap_or(false) {
        bail!("{}", ack["error"].as_str().unwrap_or("attach failed"));
    }

    let cancel = CancellationToken::new();
    let (stdin_tx, mut stdin_rx) = mpsc::unbounded_channel::<Vec<u8>>();

    let read_cancel = cancel.clone();
    let read_id = id.clone();
    let read_tx = tx.clone();
    let read_task = tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        loop {
            tokio::select! {
                () = read_cancel.cancelled() => break,
                result = reader.read(&mut buf) => {
                    match result {
                        Ok(0) | Err(_) => {
                            let _ = read_tx.send(DaemonEvent::PtyClosed { id: read_id.clone() });
                            break;
                        }
                        Ok(n) => {
                            let _ = read_tx.send(DaemonEvent::PtyBytes {
                                id: read_id.clone(),
                                bytes: buf[..n].to_vec(),
                            });
                        }
                    }
                }
            }
        }
    });

    let write_cancel = cancel.clone();
    let write_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                () = write_cancel.cancelled() => break,
                maybe_bytes = stdin_rx.recv() => {
                    let Some(bytes) = maybe_bytes else { break };
                    if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    Ok(ActiveStream {
        id,
        kind: StreamKind::Pty,
        cancel,
        stdin_tx: Some(stdin_tx),
        read_task,
        write_task: Some(write_task),
        // The first render must synchronize the daemon PTY to the real stage.
        last_size: (0, 0),
    })
}

/// Replays log lines through the same vt100 parser used for PTY output, so
/// piped, sandboxed, and exited jobs all render in an identical viewport.
async fn spawn_log_stream(
    socket_path: &Path,
    id: String,
    alive: bool,
    tx: mpsc::UnboundedSender<DaemonEvent>,
    size: (u16, u16),
) -> Result<ActiveStream> {
    let response = rpc::<serde_json::Value>(
        socket_path,
        Command::Tail(TailArgs {
            id: id.clone(),
            lines: 200,
            digest: false,
            level: None,
            strip_ansi: false,
            stream: None,
            cursor: None,
            follow: alive,
            filter_regex: None,
        }),
    )
    .await?;

    let cursor = if let Some(data) = response {
        let mut bytes = Vec::new();
        if let Some(lines) = data["lines"].as_array() {
            for line in lines {
                if let Some(content) = line["content"].as_str() {
                    bytes.extend_from_slice(content.as_bytes());
                    bytes.extend_from_slice(b"\r\n");
                }
            }
        }
        if !bytes.is_empty() {
            let _ = tx.send(DaemonEvent::PtyBytes {
                id: id.clone(),
                bytes,
            });
        }
        data["cursor"].as_u64()
    } else {
        None
    };

    let cancel = CancellationToken::new();
    let read_cancel = cancel.clone();
    let stream_id = id.clone();
    let stream_tx = tx.clone();
    let path = socket_path.to_path_buf();
    let read_task = tokio::spawn(async move {
        if !alive {
            return;
        }
        // A closed or idle daemon is expected; no need to surface it.
        let _ = stream_logs(&path, &stream_id, cursor, stream_tx, read_cancel).await;
    });

    Ok(ActiveStream {
        id,
        kind: StreamKind::Logs,
        cancel,
        stdin_tx: None,
        read_task,
        write_task: None,
        last_size: size,
    })
}

/// Streams new `LogLine`s (after `cursor`) into the viewport.
async fn stream_logs(
    socket_path: &Path,
    id: &str,
    cursor: Option<u64>,
    tx: mpsc::UnboundedSender<DaemonEvent>,
    cancel: CancellationToken,
) -> Result<()> {
    let stream = UnixStream::connect(socket_path).await?;
    let (reader, mut writer) = tokio::io::split(stream);
    let request = Request {
        id: uuid::Uuid::new_v4().to_string(),
        command: Command::StreamLogs {
            id: id.to_string(),
            cursor,
            level: None,
            filter_regex: None,
        },
    };
    let mut payload = serde_json::to_string(&request)?;
    payload.push('\n');
    writer.write_all(payload.as_bytes()).await?;
    writer.flush().await?;

    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    // Consume the control response; subsequent lines are NDJSON LogLines.
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        if !line.trim().is_empty() {
            break;
        }
    }

    loop {
        line.clear();
        tokio::select! {
            () = cancel.cancelled() => break,
            result = reader.read_line(&mut line) => {
                if result? == 0 { break; }
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim())
                    && let Some(content) = value["content"].as_str()
                {
                    let mut bytes = content.as_bytes().to_vec();
                    bytes.extend_from_slice(b"\r\n");
                    let _ = tx.send(DaemonEvent::PtyBytes { id: id.to_string(), bytes });
                }
            }
        }
    }
    Ok(())
}

/// Sends `ResizePty` when the rendered canvas size changes.
pub fn sync_pty_size(app: &App, active: &mut Option<ActiveStream>, socket_path: &Path) {
    let Some(stream) = active.as_mut() else {
        return;
    };
    if stream.kind != StreamKind::Pty {
        return;
    }
    let size = app.stage_inner_size;
    if size == stream.last_size || size.0 < 2 || size.1 < 2 {
        return;
    }
    stream.last_size = size;
    let (cols, rows) = size;
    let id = stream.id.clone();
    let path = socket_path.to_path_buf();
    tokio::spawn(async move {
        let _ = rpc::<serde_json::Value>(&path, Command::ResizePty { id, cols, rows }).await;
    });
}
