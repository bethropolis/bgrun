use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::state;

/// A single audit log entry.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AuditEntry {
    pub timestamp: String,
    pub command: String,
    pub args_summary: String,
    pub result: String,
}

/// Appends an audit entry to the audit log file.
pub async fn append(entry: AuditEntry) -> Result<()> {
    let path = state::state_dir().join("audit.log");
    let line =
        serde_json::to_string(&entry).with_context(|| "failed to serialize audit entry")? + "\n";

    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .with_context(|| format!("failed to open {}", path.display()))?
        .write_all(line.as_bytes())
        .await
        .with_context(|| "failed to write audit entry")?;

    Ok(())
}

/// Records a command invocation to the audit log.
pub async fn record(command: &str, args_summary: &str, ok: bool, error_msg: Option<&str>) {
    let result = if ok {
        "ok".to_string()
    } else {
        format!("err:{}", error_msg.unwrap_or("unknown"))
    };

    let entry = AuditEntry {
        timestamp: Utc::now().to_rfc3339(),
        command: command.to_string(),
        args_summary: args_summary.to_string(),
        result,
    };

    if let Err(e) = append(entry).await {
        tracing::error!(error = %e, "failed to write audit entry");
    }
}
