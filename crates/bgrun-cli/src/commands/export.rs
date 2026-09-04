use anyhow::Result;
use bgrun_proto::Command;

use crate::autostart::ensure_daemon_running;
use crate::client::DaemonClient;
use crate::duration::BgrunDuration;
use crate::output::output_mode;

/// Exports a job's log history (spanning rotated files) to a local file,
/// with the same filters as `tail` plus `--since`.
#[allow(clippy::too_many_arguments)]
pub async fn export(
    id: String,
    file: Option<String>,
    lines: Option<usize>,
    level: Option<String>,
    stream: Option<String>,
    strip_ansi: bool,
    filter_regex: Option<String>,
    since: Option<String>,
    json: bool,
) -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    ensure_daemon_running(&socket_path).await?;

    let mut client = DaemonClient::connect(&socket_path).await?;

    // Resolve the destination against the CLI's cwd: the daemon may run
    // with a different working directory, so it must receive an absolute
    // path to write where the user expects.
    // Char-boundary-safe truncation for the default filename (job names
    // may contain multibyte characters; byte slicing could panic).
    let short: String = id.chars().take(8).collect();
    let default_name = format!("bgrun-{short}.log");
    let dest = file.unwrap_or(default_name);
    let path = std::path::PathBuf::from(&dest);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };

    let since_ms = match since {
        Some(ref s) => Some(s.parse::<BgrunDuration>()?.0),
        None => None,
    };

    let response = client
        .send::<serde_json::Value>(Command::Export {
            id: id.clone(),
            path: path.to_string_lossy().into_owned(),
            lines,
            level,
            stream,
            strip_ansi,
            filter_regex,
            since_ms,
        })
        .await?;

    if !response.ok {
        let err = response.error.unwrap_or_default();
        anyhow::bail!("export: {err}");
    }

    if output_mode(json) == crate::output::OutputMode::Json {
        if let Some(val) = response.data {
            println!("{}", serde_json::to_string(&val)?);
        }
    } else if let Some(val) = response.data {
        let count = val.get("lines").and_then(serde_json::Value::as_u64).unwrap_or(0);
        let at = val
            .get("path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&dest);
        println!("Exported {count} lines to {at}");
    }

    Ok(())
}
