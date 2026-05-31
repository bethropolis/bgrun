use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::UnixStream;

/// Ensures the daemon is running. If the socket is missing, spawns the daemon.
pub async fn ensure_daemon_running(socket_path: &Path) -> Result<()> {
    // Try connecting first
    if UnixStream::connect(socket_path).await.is_ok() {
        return Ok(());
    }

    // Socket not available — spawn the daemon
    let daemon_bin = std::env::current_exe()
        .context("failed to get current executable path")?
        .parent()
        .context("failed to get parent dir")?
        .join("bgrun-daemon");

    std::process::Command::new(&daemon_bin)
        .spawn()
        .context("failed to spawn daemon")?;

    // Poll for socket to appear (10ms × 100 = 1s timeout)
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if UnixStream::connect(socket_path).await.is_ok() {
            return Ok(());
        }
    }

    anyhow::bail!("daemon failed to start within 1 second")
}
