use anyhow::{Context, Result};
use bgrun_proto::{LogDigest, LogLine};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};

use crate::state;

/// A single log entry written to disk as NDJSON.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DiskLogEntry {
    /// ISO 8601 timestamp (e.g. "2026-06-01T10:32:00.123Z").
    pub t: String,
    /// Stream source: "stdout", "stderr", or "pty".
    pub s: String,
    /// Log line content (without trailing newline).
    pub c: String,
}

/// Parses a log line in NDJSON format.
///
/// Expected format: `{"t":"2026-06-01T10:32:00.123Z","s":"stdout","c":"content here"}`
/// Falls back to treating the raw line as content if parsing fails
/// (e.g. for legacy logs or empty lines).
/// Returns (timestamp, stream, content).
pub fn parse_line(raw: &str) -> (Option<String>, Option<String>, String) {
    if let Ok(entry) = serde_json::from_str::<DiskLogEntry>(raw.trim()) {
        return (Some(entry.t), Some(entry.s), entry.c);
    }
    (None, None, raw.to_string())
}

/// Returns the last `n` *matching* lines from the job's stdout.log, optionally
/// filtered by stream source ("stdout", "stderr", "pty", or None for all) and/or by
/// a substring level filter (e.g. "error", "warn") and/or a regex pattern.
///
/// The file is scanned once; filters are applied during the scan so the last
/// `n` lines that *match* are returned (not the last `n` raw lines re-filtered),
/// and line numbers reflect the true position in the file.
pub async fn tail_lines(
    id: &str,
    n: usize,
    stream_filter: Option<&str>,
    level_filter: Option<&str>,
    filter_regex: Option<&regex::Regex>,
) -> Result<Vec<LogLine>> {
    let path = state::job_dir(id).join("stdout.log");
    let file = match tokio::fs::OpenOptions::new().read(true).open(&path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context("failed to open log file"),
    };

    let mut reader = BufReader::new(file);
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut line_buf = String::new();
    // Ring buffer of the last n matching lines.
    let mut matched: std::collections::VecDeque<LogLine> =
        std::collections::VecDeque::with_capacity(n);
    let mut line_number: u64 = 0;

    loop {
        line_buf.clear();
        let bytes = reader
            .read_line(&mut line_buf)
            .await
            .context("failed to read log file")?;
        if bytes == 0 {
            break;
        }
        if line_buf.ends_with('\n') {
            line_buf.pop();
            if line_buf.ends_with('\r') {
                line_buf.pop();
            }
        }
        if line_buf.is_empty() {
            // Blank line (covers both "\n" and "\r\n"): count it so numbering
            // stays accurate, but never emit it.
            line_number += 1;
            continue;
        }
        line_number += 1;

        let (timestamp, stream, content) = parse_line(&line_buf);
        if let Some(filter) = stream_filter
            && stream.as_deref() != Some(filter)
        {
            continue;
        }
        if let Some(lvl) = level_filter
            && !content.to_lowercase().contains(&lvl.to_lowercase())
        {
            continue;
        }
        if let Some(re) = filter_regex
            && !re.is_match(&content)
        {
            continue;
        }

        if matched.len() == n {
            matched.pop_front();
        }
        matched.push_back(LogLine {
            line_number,
            content,
            timestamp,
        });
    }

    Ok(matched.into_iter().collect())
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
            });
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
                    let (_, _, content) = parse_line(&line);
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
                let (_, _, content) = parse_line(&line);
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

/// Returns lines added after the given byte cursor, the new cursor position,
/// and the new line base (total lines in the live log generation).
/// Optionally filters by stream source ("stdout", "stderr", "pty", or None for all),
/// substring level filter ("error", "warn"), and/or regex pattern.
///
/// `line_base` is the caller's known line count at `cursor` (persisted per
/// diff), so numbering resumes in O(delta). A byte-cursor mismatch falls back
/// to one rescan. If `cursor` exceeds the live file (a rotation truncated
/// it), the unread tail of `stdout.log.1` is returned first.
///
/// **Line numbering is per log generation.** The returned base always counts
/// lines in the *live* file, never previous generations, so persisting it and
/// feeding it back as `line_base` stays correct across rotations. A batch that
/// crosses a rotation therefore restarts numbering at 1 for the new
/// generation, while the drained tail carries the old generation's numbers.
pub async fn diff_since(
    id: &str,
    cursor: u64,
    line_base: u64,
    stream_filter: Option<&str>,
    level_filter: Option<&str>,
    filter_regex: Option<&regex::Regex>,
) -> Result<(Vec<LogLine>, u64, u64)> {
    let dir = state::job_dir(id);
    let path = dir.join("stdout.log");
    let mut file = match tokio::fs::OpenOptions::new().read(true).open(&path).await {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Vec::new(), 0, 0)),
        Err(e) => return Err(e).context("failed to open log file"),
    };

    let file_size = file.seek(std::io::SeekFrom::End(0)).await? as u64;

    // Rotation: the caller's cursor points into a previous generation.
    // Drain the unread tail of the rotated file, then the live file.
    if cursor > file_size {
        let mut lines =
            read_rotated_tail(&dir, cursor, stream_filter, level_filter, filter_regex).await?;
        file.seek(std::io::SeekFrom::Start(0)).await?;
        let mut content = String::new();
        file.read_to_string(&mut content).await?;
        // The new line base counts the *live* generation only. Using
        // `lines.len()` here (post-filter, spanning both generations) would
        // persist a batch-local count as the cumulative base, so every
        // subsequent diff would resume numbering from a collapsed value.
        let new_base = content.lines().count() as u64;
        // Live-generation lines are numbered from 1: the base is this
        // generation's line count, matching the non-rotated path below.
        lines.extend(filter_content(
            &content,
            0,
            stream_filter,
            level_filter,
            filter_regex,
        ));
        return Ok((lines, file_size, new_base));
    }

    let offset = cursor.min(file_size);

    if offset >= file_size {
        return Ok((Vec::new(), file_size, line_base));
    }

    // Trust the persisted line base only when the byte cursor still matches;
    // otherwise (truncation, manual edit) do a single rescan.
    let line_offset = if offset == cursor {
        line_base
    } else {
        file.seek(std::io::SeekFrom::Start(0)).await?;
        let mut count = 0u64;
        let mut pos = 0u64;
        let mut buf = vec![0u8; 8192];
        while pos < offset {
            let to_read = (offset - pos).min(buf.len() as u64) as usize;
            file.read_exact(&mut buf[..to_read]).await?;
            count += buf[..to_read].iter().filter(|&&b| b == b'\n').count() as u64;
            pos += to_read as u64;
        }
        count
    };

    // Read new content from offset
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    let mut content = String::new();
    file.read_to_string(&mut content).await?;

    let lines = filter_content(
        &content,
        line_offset,
        stream_filter,
        level_filter,
        filter_regex,
    );
    let new_total = line_offset + content.lines().count() as u64;

    Ok((lines, file_size, new_total))
}

/// Reads the unread tail of `stdout.log.1` when `cursor` points past the live
/// file (a rotation happened since the last `diff`).
///
/// Lines are numbered from 1: they belong to the previous generation, whose
/// pre-rotation line count is not persisted, so an exact offset is unknowable.
async fn read_rotated_tail(
    dir: &std::path::Path,
    cursor: u64,
    stream_filter: Option<&str>,
    level_filter: Option<&str>,
    filter_regex: Option<&regex::Regex>,
) -> Result<Vec<LogLine>> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let rotated = dir.join("stdout.log.1");
    let mut file = match tokio::fs::OpenOptions::new()
        .read(true)
        .open(&rotated)
        .await
    {
        Ok(f) => f,
        // No rotated file (fresh job or cascaded away): nothing lost.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context("failed to open rotated log file"),
    };
    let size = file.seek(std::io::SeekFrom::End(0)).await? as u64;
    // `cursor` is relative to the pre-rotation live file; the exact split is
    // unknown without persisted generation sizes, so start at
    // min(cursor, size): at worst a few duplicates, never silent loss.
    let start = cursor.min(size);
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut content = String::new();
    file.take(size.saturating_sub(start))
        .read_to_string(&mut content)
        .await?;
    Ok(filter_content(
        &content,
        0,
        stream_filter,
        level_filter,
        filter_regex,
    ))
}

/// Applies stream/level/regex filters to NDJSON `content`.
fn filter_content(
    content: &str,
    line_offset: u64,
    stream_filter: Option<&str>,
    level_filter: Option<&str>,
    filter_regex: Option<&regex::Regex>,
) -> Vec<LogLine> {
    content
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let (timestamp, stream, content) = parse_line(line);
            if let Some(filter) = stream_filter
                && stream.as_deref() != Some(filter)
            {
                return None;
            }
            if let Some(lvl) = level_filter
                && !content.to_lowercase().contains(&lvl.to_lowercase())
            {
                return None;
            }
            if let Some(re) = filter_regex
                && !re.is_match(&content)
            {
                return None;
            }
            Some(LogLine {
                line_number: line_offset + i as u64 + 1,
                content,
                timestamp,
            })
        })
        .collect()
}

/// Rotation settings for a job's log files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RotationConfig {
    /// Rotate the live log once it exceeds this size in bytes.
    pub max_bytes: u64,
    /// Number of rotated files (`stdout.log.1` …) to retain.
    pub keep: u32,
}

impl Default for RotationConfig {
    fn default() -> Self {
        Self {
            max_bytes: 50 * 1024 * 1024,
            keep: 1,
        }
    }
}

impl RotationConfig {
    /// Resolves per-job overrides over the defaults. A `keep` of 0 is
    /// clamped to 1 (rotation always retains at least one generation).
    pub fn from_job(max_bytes: Option<u64>, keep: Option<u32>) -> Self {
        let defaults = Self::default();
        Self {
            max_bytes: max_bytes.unwrap_or(defaults.max_bytes),
            keep: keep.unwrap_or(defaults.keep).max(1),
        }
    }
}

/// Rotates `stdout.log` inside `dir` when it exceeds `cfg.max_bytes`,
/// cascading `stdout.log.1..=cfg.keep` so the oldest generation is dropped.
/// Returns true when a rotation happened.
pub async fn rotate_files(dir: &std::path::Path, cfg: &RotationConfig) -> Result<bool> {
    let live = dir.join("stdout.log");
    let size = match tokio::fs::metadata(&live).await {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).context("failed to stat log file"),
    };
    if size <= cfg.max_bytes {
        return Ok(false);
    }
    cascade_files(dir, cfg.keep).await?;
    tokio::fs::rename(&live, dir.join("stdout.log.1"))
        .await
        .context("failed to rotate log file")?;
    Ok(true)
}

async fn cascade_files(dir: &std::path::Path, keep: u32) -> Result<()> {
    let keep = keep.max(1);
    let _ = tokio::fs::remove_file(dir.join(format!("stdout.log.{keep}"))).await;
    for n in (1..keep).rev() {
        let from = dir.join(format!("stdout.log.{n}"));
        if tokio::fs::metadata(&from).await.is_ok() {
            tokio::fs::rename(&from, dir.join(format!("stdout.log.{}", n + 1)))
                .await
                .context("failed to cascade rotated logs")?;
        }
    }
    Ok(())
}

/// Blocking rotation check for the synchronous PTY capture loop.
/// Returns true when a rotation happened.
pub fn rotate_files_blocking(dir: &std::path::Path, cfg: &RotationConfig) -> Result<bool> {
    let live = dir.join("stdout.log");
    let size = match std::fs::metadata(&live) {
        Ok(m) => m.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e).context("failed to stat log file"),
    };
    if size <= cfg.max_bytes {
        return Ok(false);
    }
    let keep = cfg.keep.max(1);
    let _ = std::fs::remove_file(dir.join(format!("stdout.log.{keep}")));
    for n in (1..keep).rev() {
        let from = dir.join(format!("stdout.log.{n}"));
        if std::fs::metadata(&from).is_ok() {
            std::fs::rename(&from, dir.join(format!("stdout.log.{}", n + 1)))
                .context("failed to cascade rotated logs")?;
        }
    }
    std::fs::rename(&live, dir.join("stdout.log.1")).context("failed to rotate log file")?;
    Ok(true)
}

/// Backward-compatible wrapper resolving the job dir from its id.
pub async fn rotate_if_needed(id: &str, cfg: &RotationConfig) -> Result<bool> {
    rotate_files(&state::job_dir(id), cfg).await
}

/// Options for a log export (see `export_logs`).
#[derive(Debug)]
pub struct ExportOpts {
    pub out_path: std::path::PathBuf,
    pub lines: Option<usize>,
    pub level: Option<String>,
    pub stream: Option<String>,
    pub strip_ansi: bool,
    pub filter_regex: Option<regex::Regex>,
    /// Only entries newer than now-minus-this-many-ms. Entries without a
    /// parseable timestamp are kept (they cannot be proven old).
    pub since_ms: Option<u64>,
}

/// Guards `Command::Export` destinations against planting/symlink attacks.
///
/// The daemon runs with the invoking user's privileges and the socket is
/// `0600`, so same-user export targets are a legitimate feature (the CLI
/// resolves the destination against its own cwd). What must be blocked is
/// writing *through* a symlink planted elsewhere, or into a file owned by
/// a different user.
fn ensure_export_target_allowed(out_path: &std::path::Path) -> Result<()> {
    #[allow(unsafe_code)]
    fn owned_by_us(meta: &std::fs::Metadata) -> bool {
        use std::os::unix::fs::MetadataExt;
        unsafe { libc::getuid() == meta.uid() }
    }

    if let Ok(meta) = std::fs::symlink_metadata(out_path) {
        if meta.file_type().is_symlink() {
            anyhow::bail!(
                "export path {} is a symlink; refusing to write through it",
                out_path.display()
            );
        }
        if !owned_by_us(&meta) {
            anyhow::bail!(
                "export path {} is owned by another user; refusing to overwrite",
                out_path.display()
            );
        }
    }

    // Reject symlinked ancestors (e.g. a planted directory symlink).
    let mut dir = out_path.parent().unwrap_or(std::path::Path::new("/"));
    while dir != dir.parent().unwrap_or(dir) {
        if let Ok(meta) = std::fs::symlink_metadata(dir)
            && meta.file_type().is_symlink()
        {
            anyhow::bail!(
                "export parent {} is a symlink; refusing to write through it",
                dir.display()
            );
        }
        dir = dir.parent().unwrap_or(dir);
    }
    Ok(())
}

/// Writes (filtered) log content to `opts.out_path`, spanning rotated files
/// oldest-first followed by the live log. `tail`/`diff` only ever read the
/// live file; export is the way to retrieve rotated history. Returns the
/// number of lines written.
pub async fn export_logs(dir: &std::path::Path, keep: u32, opts: &ExportOpts) -> Result<usize> {
    use tokio::io::AsyncWriteExt;

    ensure_export_target_allowed(&opts.out_path)?;

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(u64::MAX, |d| {
            u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
        });
    let cutoff_ms = opts.since_ms.map(|ago| now_ms.saturating_sub(ago));

    // Oldest generation first, live log last.
    let mut sources = Vec::new();
    for n in (1..=keep.max(1)).rev() {
        let p = dir.join(format!("stdout.log.{n}"));
        if tokio::fs::metadata(&p).await.is_ok() {
            sources.push(p);
        }
    }
    sources.push(dir.join("stdout.log"));

    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        // Atomically refuse symlinks: closes the check→open race where an
        // attacker swaps the target for a symlink after validation.
        .custom_flags(libc::O_NOFOLLOW)
        .open(&opts.out_path)
        .await
        .with_context(|| format!("failed to open export file {}", opts.out_path.display()))?;

    // With a line cap we must see everything before writing, so buffer the
    // last N matches; otherwise stream straight to disk.
    let mut ring: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let cap = opts.lines.unwrap_or(usize::MAX).max(1);
    let mut written = 0usize;

    for path in sources {
        let Ok(file) = tokio::fs::File::open(&path).await else {
            continue;
        };
        let mut reader = tokio::io::BufReader::new(file);
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader
                .read_line(&mut line)
                .await
                .with_context(|| format!("failed to read {}", path.display()))?;
            if n == 0 {
                break;
            }
            let (timestamp, stream, content) = parse_line(&line);
            if !export_matches(
                &content,
                stream.as_deref(),
                timestamp.as_deref(),
                opts,
                cutoff_ms,
            ) {
                continue;
            }
            let mut text = content;
            if opts.strip_ansi {
                text = String::from_utf8_lossy(&strip_ansi_escapes::strip(text.as_bytes()))
                    .into_owned();
            }
            if opts.lines.is_some() {
                ring.push_back(text);
                if ring.len() > cap {
                    ring.pop_front();
                }
            } else {
                out.write_all(text.as_bytes()).await?;
                out.write_all(b"\n").await?;
                written += 1;
            }
        }
    }

    if opts.lines.is_some() {
        for text in ring {
            out.write_all(text.as_bytes()).await?;
            out.write_all(b"\n").await?;
            written += 1;
        }
    }
    out.flush().await?;
    Ok(written)
}

/// Shared match predicate for export filtering (same semantics as tail).
fn export_matches(
    content: &str,
    stream: Option<&str>,
    timestamp: Option<&str>,
    opts: &ExportOpts,
    cutoff_ms: Option<u64>,
) -> bool {
    if let Some(filter) = opts.stream.as_deref()
        && stream != Some(filter)
    {
        return false;
    }
    if let Some(lvl) = opts.level.as_deref()
        && !content.to_lowercase().contains(&lvl.to_lowercase())
    {
        return false;
    }
    if let Some(re) = opts.filter_regex.as_ref()
        && !re.is_match(content)
    {
        return false;
    }
    if let Some(cutoff) = cutoff_ms
        && let Some(ts) = timestamp
        && let Ok(ts_ms) = parse_ts_ms(ts)
        && ts_ms < cutoff
    {
        return false;
    }
    true
}

fn parse_ts_ms(ts: &str) -> Result<u64> {
    let dt = chrono::DateTime::parse_from_rfc3339(ts).context("invalid timestamp")?;
    Ok(dt.timestamp_millis().max(0).cast_unsigned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ndjson(t: &str, s: &str, c: &str) -> String {
        format!(r#"{{"t":"{t}","s":"{s}","c":"{c}"}}"#)
    }

    #[tokio::test]
    async fn test_rotation_config_from_job() {
        let d = RotationConfig::default();
        assert_eq!(d.max_bytes, 50 * 1024 * 1024);
        assert_eq!(d.keep, 1);
        let c = RotationConfig::from_job(Some(1024), Some(3));
        assert_eq!((c.max_bytes, c.keep), (1024, 3));
        // keep=0 clamps to 1; unset falls back to defaults.
        assert_eq!(RotationConfig::from_job(None, Some(0)).keep, 1);
        assert_eq!(RotationConfig::from_job(None, None), d);
    }

    #[tokio::test]
    async fn test_rotate_files_cascade() {
        let dir = PathBuf::from("/tmp/bgrun-test-rotate");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("stdout.log"), vec![b'x'; 100])
            .await
            .unwrap();
        tokio::fs::write(dir.join("stdout.log.1"), "old-one\n")
            .await
            .unwrap();
        tokio::fs::write(dir.join("stdout.log.2"), "old-two\n")
            .await
            .unwrap();

        let cfg = RotationConfig {
            max_bytes: 10,
            keep: 2,
        };
        assert!(rotate_files(&dir, &cfg).await.unwrap());
        // .2 now holds the previous .1, .1 holds the previous live log,
        // and the live path is gone until the capture loop reopens it.
        assert_eq!(
            tokio::fs::read_to_string(dir.join("stdout.log.2"))
                .await
                .unwrap(),
            "old-one\n"
        );
        assert_eq!(
            tokio::fs::read(&dir.join("stdout.log.1"))
                .await
                .unwrap()
                .len(),
            100
        );
        assert!(tokio::fs::metadata(dir.join("stdout.log")).await.is_err());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_rotate_files_noop_when_small() {
        let dir = PathBuf::from("/tmp/bgrun-test-rotate-noop");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        tokio::fs::write(dir.join("stdout.log"), "tiny\n")
            .await
            .unwrap();

        let cfg = RotationConfig {
            max_bytes: 10 * 1024,
            keep: 2,
        };
        assert!(!rotate_files(&dir, &cfg).await.unwrap());
        assert!(tokio::fs::metadata(dir.join("stdout.log.1")).await.is_err());
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_export_spans_rotations_with_line_cap() {
        let dir = PathBuf::from("/tmp/bgrun-test-export");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let ts = "2026-09-04T00:00:00.000Z";
        tokio::fs::write(
            dir.join("stdout.log.1"),
            format!(
                "{}\n{}\n",
                ndjson(ts, "stdout", "gen1-a"),
                ndjson(ts, "stdout", "gen1-b")
            ),
        )
        .await
        .unwrap();
        tokio::fs::write(
            dir.join("stdout.log"),
            format!(
                "{}\n{}\n",
                ndjson(ts, "stderr", "live-err"),
                ndjson(ts, "stdout", "live-ok")
            ),
        )
        .await
        .unwrap();

        let out = dir.join("out.log");
        let opts = ExportOpts {
            out_path: out.clone(),
            lines: Some(3),
            level: None,
            stream: None,
            strip_ansi: false,
            filter_regex: None,
            since_ms: None,
        };
        let written = export_logs(&dir, 2, &opts).await.unwrap();
        assert_eq!(written, 3);
        // Oldest generation first, capped to the last 3 matches.
        assert_eq!(
            tokio::fs::read_to_string(&out).await.unwrap(),
            "gen1-b\nlive-err\nlive-ok\n"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_export_filters_and_since() {
        let dir = PathBuf::from("/tmp/bgrun-test-export-filter");
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let old_ts = "2020-01-01T00:00:00.000Z";
        let new_ts = chrono::DateTime::from_timestamp_millis(now_ms as i64)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        tokio::fs::write(
            dir.join("stdout.log"),
            format!(
                "{}\n{}\n{}\n",
                ndjson(old_ts, "stdout", "ancient error"),
                ndjson(&new_ts, "stdout", "fresh error"),
                ndjson(&new_ts, "stdout", "fresh info"),
            ),
        )
        .await
        .unwrap();

        let out = dir.join("out.log");
        let opts = ExportOpts {
            out_path: out.clone(),
            lines: None,
            level: Some("error".into()),
            stream: Some("stdout".into()),
            strip_ansi: false,
            filter_regex: None,
            since_ms: Some(3_600_000), // last hour
        };
        let written = export_logs(&dir, 1, &opts).await.unwrap();
        assert_eq!(written, 1);
        assert_eq!(
            tokio::fs::read_to_string(&out).await.unwrap(),
            "fresh error\n"
        );
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_tail_lines_filtered_last_n_and_numbering() {
        let id = "tail-filter-test";
        let dir = state::job_dir(id);
        tokio::fs::create_dir_all(&dir).await.unwrap();
        let mut content = String::new();
        // 100 error lines followed by 20 info lines.
        for i in 1..=100 {
            let line = format!(
                "{{\"t\":\"2026-01-01T00:00:00.000Z\",\"s\":\"stdout\",\"c\":\"error {i}\"}}\n"
            );
            content.push_str(&line);
        }
        for i in 1..=20 {
            let line = format!(
                "{{\"t\":\"2026-01-01T00:00:00.000Z\",\"s\":\"stdout\",\"c\":\"info {i}\"}}\n"
            );
            content.push_str(&line);
        }
        tokio::fs::write(dir.join("stdout.log"), content)
            .await
            .unwrap();

        // Last 3 matching error lines — not the last 3 raw lines re-filtered.
        let lines = tail_lines(id, 3, None, Some("error"), None).await.unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].content, "error 98");
        assert_eq!(lines[2].content, "error 100");
        // True file line numbers: errors occupy lines 1..=100.
        assert_eq!(lines[0].line_number, 98);
        assert_eq!(lines[2].line_number, 100);

        // No filter: last 2 lines are the final info lines with true numbering.
        let lines = tail_lines(id, 2, None, None, None).await.unwrap();
        assert_eq!(lines[0].line_number, 119);
        assert_eq!(lines[1].line_number, 120);
        assert_eq!(lines[1].content, "info 20");

        tokio::fs::remove_dir_all(&dir).await.unwrap();
    }

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

    /// Regression: a rotation between two `diff` calls must not collapse the
    /// persisted line base. The rotation branch used to return a batch-local,
    /// post-filter count, so every later diff resumed numbering from a tiny
    /// value instead of the log's real line count.
    #[tokio::test]
    async fn test_diff_since_line_base_survives_rotation() {
        let id = "test-diff-rotation-base";
        let job_dir = crate::state::state_dir().join("jobs").join(id);
        let _ = tokio::fs::remove_dir_all(&job_dir).await;
        tokio::fs::create_dir_all(&job_dir).await.unwrap();
        let log = job_dir.join("stdout.log");

        let gen1: String = std::iter::repeat_n("g1\n", 5).collect();
        tokio::fs::write(&log, &gen1).await.unwrap();

        // First diff: everything, base should be the live generation's count.
        let (lines, cursor, base) = diff_since(id, 0, 0, None, None, None).await.unwrap();
        assert_eq!(lines.len(), 5);
        assert_eq!(base, 5);
        assert_eq!(cursor, gen1.len() as u64);

        // Rotate: the generation we were reading becomes stdout.log.1 and a
        // new, smaller live log appears (cursor now exceeds file_size).
        tokio::fs::rename(&log, job_dir.join("stdout.log.1"))
            .await
            .unwrap();
        let gen2: String = std::iter::repeat_n("g2\n", 2).collect();
        tokio::fs::write(&log, &gen2).await.unwrap();

        let (lines, cursor, base) = diff_since(id, cursor, base, None, None, None)
            .await
            .unwrap();
        // Drained old tail plus the whole new generation.
        assert_eq!(lines.len(), 2, "rotated tail past cursor is empty here");
        assert_eq!(
            base, 2,
            "line base must count the live generation, not the batch"
        );

        // Append to the live log: numbering must continue from the base
        // (3, 4) rather than restarting from a collapsed value.
        tokio::fs::write(&log, format!("{gen2}g2x\ng2y\n"))
            .await
            .unwrap();
        let (lines, _cursor, base) = diff_since(id, cursor, base, None, None, None)
            .await
            .unwrap();
        assert_eq!(base, 4);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].line_number, 3);
        assert_eq!(lines[0].content, "g2x");
        assert_eq!(lines[1].line_number, 4);

        let _ = tokio::fs::remove_dir_all(&job_dir).await;
    }

    #[tokio::test]
    async fn test_tail_lines_with_level_filter() {
        // Create a real job directory under the daemon state path
        let state_dir = crate::state::state_dir();
        let job_dir = state_dir.join("jobs").join("test-level");
        let _ = tokio::fs::create_dir_all(&job_dir).await;
        let log = job_dir.join("stdout.log");
        let _ = tokio::fs::write(
            &log,
            "info: starting\nwarn: low disk\nerror: oops\ninfo: done\n",
        )
        .await;

        let result = diff_since("test-level", 0, 0, None, Some("error"), None).await;
        let _ = tokio::fs::remove_dir_all(&job_dir).await;

        let (lines, _cursor, _) = result.expect("diff_since should succeed");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].content.contains("error"));
    }

    #[tokio::test]
    async fn test_tail_lines_with_regex_filter() {
        let state_dir = crate::state::state_dir();
        let job_dir = state_dir.join("jobs").join("test-regex");
        let _ = tokio::fs::create_dir_all(&job_dir).await;
        let log = job_dir.join("stdout.log");
        let _ = tokio::fs::write(&log, "line1\nport=8080\nline3\nport=9090\n").await;

        let re = regex::Regex::new("port=\\d+").ok();
        let result = diff_since("test-regex", 0, 0, None, None, re.as_ref()).await;
        let _ = tokio::fs::remove_dir_all(&job_dir).await;

        let (lines, _cursor, _) = result.expect("diff_since should succeed");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].content.contains("8080"));
        assert!(lines[1].content.contains("9090"));
    }
}
