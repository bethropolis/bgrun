use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use bgrun_core::JobStore;
use bgrun_proto::{JobState, ReadinessStrategy};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Async trait for checking whether a job is ready.
#[async_trait]
pub trait ReadinessChecker: Send + Sync {
    /// Returns true when the job is ready.
    async fn check(&self) -> bool;
    /// Human-readable description of this readiness check.
    fn description(&self) -> String;
}

/// Checks if a log file contains a substring pattern.
///
/// Tracks the byte offset of the last read so each check only scans new bytes.
pub struct LogPatternChecker {
    path: PathBuf,
    pattern: String,
    offset: Arc<Mutex<u64>>,
}

impl LogPatternChecker {
    /// Creates a checker that scans the log file for the given pattern.
    pub fn new(log_path: PathBuf, pattern: String) -> Self {
        LogPatternChecker {
            path: log_path,
            pattern,
            offset: Arc::new(Mutex::new(0)),
        }
    }
}

#[async_trait]
impl ReadinessChecker for LogPatternChecker {
    async fn check(&self) -> bool {
        let mut file = match tokio::fs::OpenOptions::new()
            .read(true)
            .open(&self.path)
            .await
        {
            Ok(f) => f,
            Err(_) => return false,
        };

        let mut offset = self.offset.lock().await;
        let start = *offset;

        // Seek to where we left off
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return false;
        }

        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).await.is_err() {
            return false;
        }

        let new_bytes = String::from_utf8_lossy(&buf);
        let found = new_bytes.contains(&self.pattern);

        // Update offset to end of what we just read
        *offset = start + buf.len() as u64;

        found
    }

    fn description(&self) -> String {
        format!("log pattern '{}'", self.pattern)
    }
}

/// Checks if a log file contains a regex pattern.
///
/// Tracks the byte offset of the last read so each check only scans new bytes.
pub struct RegexPatternChecker {
    path: PathBuf,
    pattern: regex::Regex,
    offset: Arc<Mutex<u64>>,
}

impl RegexPatternChecker {
    /// Creates a checker that scans the log file for the given regex pattern.
    pub fn new(log_path: PathBuf, pattern: regex::Regex) -> Self {
        RegexPatternChecker {
            path: log_path,
            pattern,
            offset: Arc::new(Mutex::new(0)),
        }
    }
}

#[async_trait]
impl ReadinessChecker for RegexPatternChecker {
    async fn check(&self) -> bool {
        let mut file = match tokio::fs::OpenOptions::new()
            .read(true)
            .open(&self.path)
            .await
        {
            Ok(f) => f,
            Err(_) => return false,
        };

        let mut offset = self.offset.lock().await;
        let start = *offset;

        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return false;
        }

        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).await.is_err() {
            return false;
        }

        let new_bytes = String::from_utf8_lossy(&buf);
        let found = self.pattern.is_match(&new_bytes);

        *offset = start + buf.len() as u64;

        found
    }

    fn description(&self) -> String {
        format!("log regex '{}'", self.pattern.as_str())
    }
}

/// Checks if a TCP port is connectable (async).
pub struct TcpPortChecker {
    port: u16,
}

impl TcpPortChecker {
    /// Creates a checker for the given TCP port.
    pub fn new(port: u16) -> Self {
        TcpPortChecker { port }
    }
}

#[async_trait]
impl ReadinessChecker for TcpPortChecker {
    async fn check(&self) -> bool {
        let addr = format!("127.0.0.1:{}", self.port);
        TcpStream::connect(&addr).await.is_ok()
    }

    fn description(&self) -> String {
        format!("TCP port {}", self.port)
    }
}

/// Checks if an HTTP endpoint returns 2xx.
pub struct HttpPollChecker {
    url: String,
    client: reqwest::Client,
}

impl HttpPollChecker {
    /// Creates a checker that polls the given URL.
    pub fn new(url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(500))
            .build()
            .unwrap_or_default();
        HttpPollChecker { url, client }
    }
}

#[async_trait]
impl ReadinessChecker for HttpPollChecker {
    async fn check(&self) -> bool {
        match self.client.get(&self.url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    fn description(&self) -> String {
        format!("HTTP poll '{}'", self.url)
    }
}

/// Checks if a file exists at a path.
pub struct FileExistsChecker {
    path: PathBuf,
}

impl FileExistsChecker {
    /// Creates a checker for file existence.
    pub fn new(path: PathBuf) -> Self {
        FileExistsChecker { path }
    }
}

#[async_trait]
impl ReadinessChecker for FileExistsChecker {
    async fn check(&self) -> bool {
        tokio::fs::metadata(&self.path).await.is_ok()
    }

    fn description(&self) -> String {
        format!("file '{}'", self.path.display())
    }
}

/// Builds the appropriate readiness checker for a given strategy.
pub fn build_checker(
    strategy: &ReadinessStrategy,
    job_dir: &std::path::Path,
) -> Box<dyn ReadinessChecker> {
    match strategy {
        ReadinessStrategy::LogPattern(pattern) => Box::new(LogPatternChecker::new(
            job_dir.join("stdout.log"),
            pattern.clone(),
        )),
        ReadinessStrategy::LogPatternRegex(pattern) => {
            match regex::Regex::new(pattern) {
                Ok(re) => Box::new(RegexPatternChecker::new(
                    job_dir.join("stdout.log"),
                    re,
                )),
                Err(e) => {
                    warn!(error = %e, "invalid readiness regex pattern, falling back to substring match");
                    Box::new(LogPatternChecker::new(
                        job_dir.join("stdout.log"),
                        pattern.clone(),
                    ))
                }
            }
        }
        ReadinessStrategy::TcpPort(port) => Box::new(TcpPortChecker::new(*port)),
        ReadinessStrategy::HttpPoll(url) => Box::new(HttpPollChecker::new(url.clone())),
        ReadinessStrategy::FileExists(path) => {
            Box::new(FileExistsChecker::new(PathBuf::from(path)))
        }
    }
}

/// Polls a readiness checker and transitions the job to Ready when it fires.
///
/// Exits early if the job dies (not alive) before becoming ready.
pub async fn readiness_loop(
    id: String,
    store: Arc<Mutex<JobStore>>,
    checker: Box<dyn ReadinessChecker>,
    timeout_ms: u64,
) {
    let start = tokio::time::Instant::now();
    let timeout = std::time::Duration::from_millis(timeout_ms);

    info!(
        id = %id,
        checker = checker.description(),
        "readiness check started"
    );

    loop {
        if start.elapsed() >= timeout {
            warn!(id = %id, "readiness check timed out");
            return;
        }

        // Exit early if the job is no longer alive
        {
            let store_ref = store.lock().await;
            match store_ref.get(&id) {
                Some(job) if job.is_alive() => {}
                _ => {
                    info!(id = %id, "readiness check stopped: job no longer alive");
                    return;
                }
            }
        }

        if checker.check().await {
            let elapsed = start.elapsed().as_millis() as u64;
            let mut store = store.lock().await;
            if let Some(job) = store.get_mut(&id) {
                if let Ok(()) = job.transition(JobState::Ready) {
                    job.ready_at = Some(chrono::Utc::now());
                    job.consecutive_failures = 0;
                    let _ = crate::state::write_status(job).await;
                    info!(
                        id = %id,
                        elapsed_ms = %elapsed,
                        "job became ready"
                    );
                    return;
                }
            }
            return;
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Port conflict pre-check: tries to connect immediately (async).
pub async fn check_port_available(port: u16) -> bool {
    let addr = format!("127.0.0.1:{}", port);
    TcpStream::connect(&addr).await.is_err()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn test_port_available_when_free() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        // Test with sync std::net since this is a sync test
        assert!(std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_err());
    }

    #[test]
    fn test_port_unavailable_when_bound() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok());
        drop(listener);
    }

    #[tokio::test]
    async fn test_file_exists_checker_missing() {
        let checker = FileExistsChecker::new(PathBuf::from("/tmp/bgrun-test-nonexistent"));
        assert!(!checker.check().await);
    }

    #[tokio::test]
    async fn test_file_exists_checker_present() {
        let path = PathBuf::from("/tmp/bgrun-test-existing");
        let _ = tokio::fs::write(&path, "test").await;
        let checker = FileExistsChecker::new(path.clone());
        assert!(checker.check().await);
        let _ = tokio::fs::remove_file(&path).await;
    }

    #[tokio::test]
    async fn test_log_pattern_checker_matches() {
        let dir = PathBuf::from("/tmp/bgrun-test-log");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let log_path = dir.join("stdout.log");
        let _ = tokio::fs::write(&log_path, "line 1\nlistening on :8080\nline 3\n").await;

        let checker = LogPatternChecker::new(log_path.clone(), "listening on".into());
        assert!(checker.check().await);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_log_pattern_checker_no_match() {
        let dir = PathBuf::from("/tmp/bgrun-test-log-nomatch");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let log_path = dir.join("stdout.log");
        let _ = tokio::fs::write(&log_path, "line 1\nline 2\n").await;

        let checker = LogPatternChecker::new(log_path.clone(), "never_match".into());
        assert!(!checker.check().await);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_log_pattern_checker_missing_file() {
        let checker = LogPatternChecker::new(
            PathBuf::from("/tmp/bgrun-test-nonexistent-log"),
            "pattern".into(),
        );
        assert!(!checker.check().await);
    }

    #[tokio::test]
    async fn test_log_pattern_checker_offset_tracking() {
        let dir = PathBuf::from("/tmp/bgrun-test-log-offset");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let log_path = dir.join("stdout.log");

        // Write first chunk
        let _ = tokio::fs::write(&log_path, "line1\nline2\n").await;
        let checker = LogPatternChecker::new(log_path.clone(), "target".into());

        // First check: no match
        assert!(!checker.check().await);

        // Append target line
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .await
            .unwrap();
        file.write_all(b"target found\n").await.unwrap();
        drop(file);

        // Second check: should find it (only scans new bytes)
        assert!(checker.check().await);

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn test_build_checker_pattern() {
        let strategy = ReadinessStrategy::LogPattern("ready".into());
        let job_dir = PathBuf::from("/tmp/test-job");
        let _checker = build_checker(&strategy, &job_dir);
    }

    #[test]
    fn test_build_checker_port() {
        let strategy = ReadinessStrategy::TcpPort(3000);
        let job_dir = PathBuf::from("/tmp/test-job");
        let _checker = build_checker(&strategy, &job_dir);
    }

    #[test]
    fn test_build_checker_http() {
        let strategy = ReadinessStrategy::HttpPoll("http://localhost:3000/health".into());
        let job_dir = PathBuf::from("/tmp/test-job");
        let _checker = build_checker(&strategy, &job_dir);
    }

    #[test]
    fn test_build_checker_file() {
        let strategy = ReadinessStrategy::FileExists("/tmp/ready".into());
        let job_dir = PathBuf::from("/tmp/test-job");
        let _checker = build_checker(&strategy, &job_dir);
    }
}
