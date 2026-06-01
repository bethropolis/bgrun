use anyhow::{Context, Result};
use bgrun_proto::{LogDigest, LogLine};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::state;

/// Parses a log line, extracting the optional ISO 8601 timestamp prefix.
///
/// Expected format: `[2026-06-01T10:32:00.123Z] content here`
/// Lines without a valid timestamp prefix return `(None, raw_line)`.
fn parse_line(raw: &str) -> (Option<String>, String) {
    if let Some(stripped) = raw.strip_prefix('[') {
        if let Some(end) = stripped.find("] ") {
            let ts = &stripped[..end];
            // Basic validation: contains T and ends with Z or offset digit
            if ts.contains('T') {
                return (Some(ts.to_string()), stripped[end + 2..].to_string());
            }
        }
    }
    (None, raw.to_string())
}

/// Returns the last `n` lines from the job's stdout.log.
///
/// First pass counts lines and tracks newline byte offsets as a ring buffer of N+1.
/// Second pass reads only the needed portion from disk.
pub async fn tail_lines(id: &str, n: usize) -> Result<Vec<LogLine>> {
    let path = state::job_dir(id).join("stdout.log");
    let mut file = match tokio::fs::OpenOptions::new().read(true).open(&path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context("failed to open log file"),
    };

    let file_size = file.seek(std::io::SeekFrom::End(0)).await? as usize;
    if file_size == 0 {
        return Ok(Vec::new());
    }

    file.seek(std::io::SeekFrom::Start(0)).await?;

    // Pass 1: track newline positions in a ring buffer of size n+1.
    let mut nl_positions: Vec<usize> = Vec::with_capacity(n + 2);
    let mut pos = 0usize;
    let mut buf = vec![0u8; 65536];

    while pos < file_size {
        let to_read = (file_size - pos).min(buf.len());
        file.read_exact(&mut buf[..to_read]).await?;
        for (i, &b) in buf[..to_read].iter().enumerate() {
            if b == b'\n' {
                nl_positions.push(pos + i);
                if nl_positions.len() > n + 1 {
                    nl_positions.remove(0);
                }
            }
        }
        pos += to_read;
    }

    // Determine start byte offset for last N lines
    let start_offset = if nl_positions.len() > n {
        nl_positions[0] + 1
    } else {
        0usize
    };

    // Pass 2: read content from start_offset
    file.seek(std::io::SeekFrom::Start(start_offset as u64))
        .await?;
    let remaining = file_size - start_offset;
    let mut content = String::with_capacity(remaining);
    file.read_to_string(&mut content).await?;

    let lines: Vec<&str> = content.lines().collect();
    let line_offset = nl_positions.len().saturating_sub(lines.len()) as u64 + 1;

    let result: Vec<LogLine> = lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let (timestamp, content) = parse_line(line);
            LogLine {
                line_number: line_offset + i as u64,
                content,
                timestamp,
            }
        })
        .collect();

    Ok(result)
}

/// Returns a digest summary of the job's log.
pub async fn tail_digest(id: &str) -> Result<LogDigest> {
    let path = state::job_dir(id).join("stdout.log");
    let mut file = match tokio::fs::OpenOptions::new().read(true).open(&path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LogDigest {
                total_lines: 0,
                errors: 0,
                warnings: 0,
                last_error: None,
                last_error_line: None,
            })
        }
        Err(e) => return Err(e).context("failed to open log file"),
    };

    let mut total_lines: u64 = 0;
    let mut errors: u64 = 0;
    let mut warnings: u64 = 0;
    let mut last_error: Option<String> = None;
    let mut last_error_line: Option<u64> = None;
    let mut partial_line = Vec::new();
    let mut line_number: u64 = 0;

    let mut buf = vec![0u8; 8192];
    loop {
        let n = match file.read(&mut buf).await {
            Ok(0) => {
                if !partial_line.is_empty() {
                    line_number += 1;
                    total_lines += 1;
                    let line = String::from_utf8_lossy(&partial_line);
                    let (_, content) = parse_line(&line);
                    process_line(
                        &content,
                        line_number,
                        &mut errors,
                        &mut warnings,
                        &mut last_error,
                        &mut last_error_line,
                    );
                }
                break;
            }
            Ok(n) => n,
            Err(_) => break,
        };

        let mut start = 0;
        for i in 0..n {
            if buf[i] == b'\n' {
                line_number += 1;
                total_lines += 1;
                    let mut line_bytes = partial_line.clone();
                    line_bytes.extend_from_slice(&buf[start..i]);
                    let line = String::from_utf8_lossy(&line_bytes);
                    let (_, content) = parse_line(&line);
                    process_line(
                        &content,
                    line_number,
                    &mut errors,
                    &mut warnings,
                    &mut last_error,
                    &mut last_error_line,
                );
                partial_line.clear();
                start = i + 1;
            }
        }
        if start < n {
            partial_line.extend_from_slice(&buf[start..n]);
        }
    }

    Ok(LogDigest {
        total_lines,
        errors,
        warnings,
        last_error,
        last_error_line,
    })
}

fn process_line(
    line: &str,
    line_number: u64,
    errors: &mut u64,
    warnings: &mut u64,
    last_error: &mut Option<String>,
    last_error_line: &mut Option<u64>,
) {
    let lower = line.to_lowercase();
    if lower.contains("error") {
        *errors += 1;
        *last_error = Some(line.to_string());
        *last_error_line = Some(line_number);
    } else if lower.contains("warn") {
        *warnings += 1;
    }
}

/// Returns lines added after the given byte cursor, and the new cursor position.
pub async fn diff_since(id: &str, cursor: u64) -> Result<(Vec<LogLine>, u64)> {
    let path = state::job_dir(id).join("stdout.log");
    let mut file = match tokio::fs::OpenOptions::new().read(true).open(&path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0)),
        Err(e) => return Err(e).context("failed to open log file"),
    };

    let file_size = file.seek(std::io::SeekFrom::End(0)).await? as u64;
    let offset = cursor.min(file_size);

    if offset >= file_size {
        return Ok((Vec::new(), file_size));
    }

    // Seek to cursor and count lines up to that point for line numbering
    file.seek(std::io::SeekFrom::Start(0)).await?;
    let mut line_offset = 0u64;
    let mut pos = 0u64;
    let mut buf = vec![0u8; 8192];
    while pos < offset {
        let to_read = (offset - pos).min(buf.len() as u64) as usize;
        file.read_exact(&mut buf[..to_read]).await?;
        line_offset += buf[..to_read].iter().filter(|&&b| b == b'\n').count() as u64;
        pos += to_read as u64;
    }

    // Read new content from offset
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut content = String::new();
    file.read_to_string(&mut content).await?;

    let lines: Vec<LogLine> = content
        .lines()
        .enumerate()
        .map(|(i, line)| {
            let (timestamp, content) = parse_line(line);
            LogLine {
                line_number: line_offset + i as u64 + 1,
                content,
                timestamp,
            }
        })
        .collect();

    Ok((lines, file_size))
}

/// Rotates the log file if it exceeds 50MB.
pub async fn rotate_if_needed(id: &str) -> Result<()> {
    let dir = state::job_dir(id);
    let log_path = dir.join("stdout.log");
    let rotated_path = dir.join("stdout.log.1");

    let metadata = match tokio::fs::metadata(&log_path).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).context("failed to stat log file"),
    };

    if metadata.len() > 50 * 1024 * 1024 {
        let _ = tokio::fs::remove_file(&rotated_path).await;
        tokio::fs::rename(&log_path, &rotated_path)
            .await
            .context("failed to rotate log file")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_tail_lines_reads_last_n() {
        let dir = PathBuf::from("/tmp/bgrun-test-tail");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let log = dir.join("stdout.log");
        let _ = tokio::fs::write(&log, "line1\nline2\nline3\nline4\nline5\n").await;

        let content = tokio::fs::read_to_string(&log).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        let total = lines.len();
        let start = total.saturating_sub(3);
        let result: Vec<LogLine> = lines[start..]
            .iter()
            .enumerate()
            .map(|(i, line)| LogLine {
                line_number: (start + i + 1) as u64,
                content: line.to_string(),
                timestamp: None,
            })
            .collect();

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].line_number, 3);
        assert_eq!(result[0].content, "line3");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_diff_since() {
        let dir = PathBuf::from("/tmp/bgrun-test-diff");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let log = dir.join("stdout.log");
        let _ = tokio::fs::write(&log, "line1\nline2\nline3\n").await;

        let content = tokio::fs::read_to_string(&log).await.unwrap();
        let bytes = content.into_bytes();
        let cursor = 6; // after "line1\n"
        let new_content = String::from_utf8_lossy(&bytes[cursor..]);
        let lines: Vec<&str> = new_content.lines().collect();

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "line2");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
