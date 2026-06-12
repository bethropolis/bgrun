use std::sync::Arc;

use anyhow::{Context, Result};
use bgrun_core::JobStore;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing_subscriber::EnvFilter;

mod runner;
mod server;
mod state;

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var_os("BGRUN_DAEMONIZED").is_none() {
        spawn_detached_daemon().context("failed to spawn detached daemon")?;
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_env("BGRUN_LOG"))
        .init();

    let socket_path = state::socket_path();
    let daemon_dir = state::state_dir();

    // Ensure state directory exists
    tokio::fs::create_dir_all(&daemon_dir).await?;
    tokio::fs::write(
        daemon_dir.join("daemon.pid"),
        std::process::id().to_string(),
    )
    .await?;

    let mut initial_store = JobStore::new();
    for job in state::read_all_jobs().await? {
        initial_store.insert(job);
    }
    let store = Arc::new(Mutex::new(initial_store));

    // Re-adopt orphaned jobs from previous daemon instance
    if let Err(e) = bgrun_daemon::orphan::readopt_all(store.clone()).await {
        tracing::warn!(error = %e, "orphan re-adoption failed");
    }

    // Create shutdown token for coordinated cancellation
    let shutdown = CancellationToken::new();

    // Signal handler: catch SIGTERM, SIGINT (Ctrl+C), SIGHUP
    let sig_shutdown = shutdown.clone();
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};

        let mut term = signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("failed to register SIGINT handler");
        let mut hup = signal(SignalKind::hangup()).expect("failed to register SIGHUP handler");

        tokio::select! {
            _ = term.recv() => tracing::info!("received SIGTERM"),
            _ = int.recv() => tracing::info!("received SIGINT"),
            _ = hup.recv() => tracing::info!("received SIGHUP"),
        }

        tracing::info!("shutdown signal received, killing all jobs");
        sig_shutdown.cancel();
    });

    // Spawn the reactive, event-driven auto-shutdown monitor (0% idle polling)
    let store_clone = store.clone();
    let socket_path_clone = socket_path.clone();
    let shutdown_monitor = shutdown.clone();
    tokio::spawn(async move {
        use crate::runner::LIFECYCLE_NOTIFY;

        let timeout_seconds = std::env::var("BGRUN_IDLE_TIMEOUT")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(60);

        tracing::info!(
            timeout_secs = timeout_seconds,
            "reactive auto-shutdown monitor initialized (0% idle polling)"
        );

        loop {
            tokio::select! {
                biased;
                _ = shutdown_monitor.cancelled() => {
                    tracing::info!("shutdown token set, auto-shutdown monitor exiting");
                    break;
                }
                _ = async {
                    let active_count = {
                        let store_ref = store_clone.lock().await;
                        store_ref
                            .list_workspace(None)
                            .iter()
                            .filter(|j| j.is_alive())
                            .count()
                    };

                    if active_count > 0 {
                        LIFECYCLE_NOTIFY.notified().await;
                    } else {
                        tokio::select! {
                            _ = LIFECYCLE_NOTIFY.notified() => {}
                            _ = tokio::time::sleep(std::time::Duration::from_secs(timeout_seconds)) => {
                                let final_count = {
                                    let store_ref = store_clone.lock().await;
                                    store_ref
                                        .list_workspace(None)
                                        .iter()
                                        .filter(|j| j.is_alive())
                                        .count()
                                };
                                if final_count == 0 {
                                    tracing::info!(
                                        timeout_secs = timeout_seconds,
                                        "idle timeout reached reactively, initiating graceful auto-shutdown"
                                    );
                                    let _ = tokio::fs::remove_file(&socket_path_clone).await;
                                    std::process::exit(0);
                                }
                            }
                        }
                    }
                } => {}
            }
        }
    });

    info!(
        socket = %socket_path.display(),
        dir = %daemon_dir.display(),
        "daemon starting"
    );

    // Run server until shutdown is signalled
    server::run_server(socket_path.clone(), store.clone(), shutdown.clone()).await?;

    tracing::info!("server stopped, cleaning up jobs");
    runner::kill_all_jobs(store).await;
    let _ = tokio::fs::remove_file(&socket_path).await;
    tracing::info!("daemon shut down complete");
    Ok(())
}

/// Spawns this executable as a detached daemon process.
fn spawn_detached_daemon() -> Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe().context("failed to resolve daemon executable")?;
    let log_path = state::state_dir().join("daemon.log");
    std::fs::create_dir_all(state::state_dir()).context("failed to create state directory")?;

    let mut command = detached_command(&exe, &log_path)?;
    unsafe {
        command.pre_exec(|| {
            nix::unistd::setsid().map_err(std::io::Error::other)?;
            Ok(())
        });
    }

    match command.spawn() {
        Ok(_) => Ok(()),
        Err(err) if err.raw_os_error() == Some(nix::libc::EPERM) => {
            detached_command(&exe, &log_path)?
                .spawn()
                .context("failed to spawn daemon child without setsid fallback")?;
            Ok(())
        }
        Err(err) => Err(err).context("failed to spawn daemon child"),
    }
}

/// Builds a daemon child command with stdio detached from the caller.
fn detached_command(
    exe: &std::path::Path,
    log_path: &std::path::Path,
) -> Result<std::process::Command> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("failed to open {}", log_path.display()))?;
    let log_stderr = log
        .try_clone()
        .with_context(|| format!("failed to clone {}", log_path.display()))?;
    let stdin = std::fs::File::open("/dev/null").context("failed to open /dev/null")?;

    let mut command = std::process::Command::new(exe);
    command
        .env("BGRUN_DAEMONIZED", "1")
        .stdin(stdin)
        .stdout(log)
        .stderr(log_stderr);

    Ok(command)
}
