use anyhow::{Context, Result};
use bgrun_proto::Command;
use crossterm::event::{Event, EventStream};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use futures_util::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

const ESCAPE_P: u8 = 0x10;
const ESCAPE_Q: u8 = 0x11;
/// Attach to a PTY job's stdin/stdout interactively.
///
/// Ctrl+P then Ctrl+Q detaches the client (the job continues running).
pub async fn attach_job(id: String, _json: bool) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();

    // Ensure daemon is running
    crate::autostart::ensure_daemon_running(&socket_path).await?;

    // Connect directly (no DaemonClient — we need the raw stream)
    let stream = UnixStream::connect(&socket_path)
        .await
        .context("failed to connect to daemon socket")?;

    // Send NDJSON attach request (use write half, then split)
    let request = bgrun_proto::Request {
        id: uuid::Uuid::new_v4().to_string(),
        command: Command::Attach { id: id.clone() },
    };
    let json = serde_json::to_string(&request)?;

    // Write request, then wrap in BufReader for response reading
    let (mut read_half, mut write_half) = tokio::io::split(stream);
    write_half.write_all(json.as_bytes()).await?;
    write_half.write_all(b"\n").await?;
    write_half.flush().await?;

    // Read initial response line
    let mut response_buf = String::new();
    {
        let mut reader = BufReader::new(&mut read_half);
        reader.read_line(&mut response_buf).await?;
    }

    let response: serde_json::Value = serde_json::from_str(response_buf.trim())?;
    if response.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let err = response
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("attach failed");
        anyhow::bail!("{}", err);
    }

    // Notify user of interactive instructions before switching terminal modes
    println!(
        "Attaching to job '{}'. Press Ctrl+P followed by Ctrl+Q to detach.",
        id
    );

    // Enable raw mode with safety guard (restores terminal even on panic)
    let _guard = RawModeGuard::new()?;

    let result = run_attach_loop(id, read_half, write_half).await;
    result
}

async fn run_attach_loop(
    id: String,
    mut stream_read: tokio::io::ReadHalf<UnixStream>,
    mut stream_write: tokio::io::WriteHalf<UnixStream>,
) -> Result<()> {
    // Spawn task to forward socket output → stdout
    let stdout_task: tokio::task::JoinHandle<Result<()>> = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        let mut buf = [0u8; 8192];
        loop {
            match stream_read.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    stdout.write_all(&buf[..n]).await?;
                    stdout.flush().await?;
                }
                Err(_) => break,
            }
        }
        Ok(())
    });

    // Spawn background task to monitor terminal window resizing
    let id_clone = id.clone();
    let resize_task: tokio::task::JoinHandle<()> = tokio::spawn(async move {
        let mut reader = EventStream::new();
        let socket_path = bgrun_proto::paths::socket_path();

        while let Some(Ok(event)) = reader.next().await {
            if let Event::Resize(cols, rows) = event {
                if let Ok(mut client) = crate::client::DaemonClient::connect(&socket_path).await {
                    let _ = client
                        .send::<serde_json::Value>(bgrun_proto::Command::ResizePty {
                            id: id_clone.clone(),
                            cols,
                            rows,
                        })
                        .await;
                }
            }
        }
    });

    // In main task: forward stdin → socket, detect escape sequence
    let stdin_result = forward_stdin(&mut stream_write).await;

    // Shutdown tasks cleanly
    stdout_task.abort();
    resize_task.abort();
    let _ = stdout_task.await;
    let _ = resize_task.await;
    let _ = stream_write.shutdown().await;

    stdin_result
}

/// Reads bytes from stdin and writes them to the socket.
/// Ctrl+P then Ctrl+Q detaches (returns Ok). All other bytes including
/// Ctrl+C (0x03) are forwarded to the PTY so the remote process receives
/// real terminal signals.
async fn forward_stdin(writer: &mut tokio::io::WriteHalf<UnixStream>) -> Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut buf = [0u8; 1024];
    let mut ctrl_p_pressed = false;

    loop {
        match stdin.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let mut write_buf = Vec::with_capacity(n);
                for &b in &buf[..n] {
                    if ctrl_p_pressed {
                        if b == ESCAPE_Q {
                            return Ok(());
                        } else {
                            write_buf.push(ESCAPE_P);
                            write_buf.push(b);
                            ctrl_p_pressed = false;
                        }
                    } else if b == ESCAPE_P {
                        ctrl_p_pressed = true;
                    } else {
                        write_buf.push(b);
                    }
                }

                if !write_buf.is_empty() {
                    writer.write_all(&write_buf).await?;
                    writer.flush().await?;
                }
            }
            Err(_) => break,
        }
    }

    // Flush any trailing escape character if stdin ended abruptly
    if ctrl_p_pressed {
        writer.write_all(&[ESCAPE_P]).await?;
        writer.flush().await?;
    }

    Ok(())
}

/// RAII guard that restores the terminal to cooked mode on drop.
/// Ensures raw mode is cleaned up even on panic or early return.
struct RawModeGuard;

impl RawModeGuard {
    fn new() -> Result<Self> {
        enable_raw_mode().context("failed to enable raw mode")?;
        Ok(RawModeGuard)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}
