//! cgroups v2 resource limits for jobs (Linux only).
//!
//! Jobs run in a per-job group (`bgrun/<job-id>`) under either the system
//! hierarchy (root / delegated) or the caller's systemd user slice
//! (rootless). Limits are hard kernel enforcement, unlike the portable
//! `--max-rss` poll-and-kill fallback.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Resource limits applied to a job's cgroup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Limits {
    /// `memory.max` in bytes.
    pub memory_max: Option<u64>,
    /// `cpu.max` quota as percent of one CPU (50 = half a CPU; >100 spans
    /// multiple CPUs).
    pub cpu_quota_pct: Option<u32>,
    /// `pids.max` (max tasks in the group).
    pub pids_max: Option<u32>,
}

impl Limits {
    /// True when no limit is set (nothing to enforce).
    pub fn is_empty(&self) -> bool {
        self.memory_max.is_none() && self.cpu_quota_pct.is_none() && self.pids_max.is_none()
    }

    /// Renders the `cpu.max` file content for a percent quota:
    /// `<quota> 100000` (100ms period).
    pub fn cpu_max_content(pct: u32) -> String {
        format!("{} 100000", u64::from(pct).saturating_mul(1000))
    }
}

/// Creates a per-job group, applies `limits`, and moves `pid` into it.
/// Tries the system base first, then the rootless user-slice base.
/// Fails loudly when neither is usable — callers must NOT silently run
/// limited jobs unlimited (use `--max-rss` for best-effort polling instead).
pub fn setup_job_group(job_id: &str, pid: u32, limits: &Limits) -> Result<PathBuf> {
    let uid = nix::unistd::getuid();
    let candidates = [
        PathBuf::from("/sys/fs/cgroup/bgrun"),
        PathBuf::from(format!(
            "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/bgrun"
        )),
    ];
    let mut errors = Vec::new();
    for base in &candidates {
        match setup_in_base(base, job_id, pid, limits) {
            Ok(path) => {
                tracing::info!(job_id = %job_id, group = %path.display(), "cgroup limits applied");
                return Ok(path);
            }
            Err(e) => errors.push(format!("{}: {e:#}", base.display())),
        }
    }
    Err(anyhow::anyhow!(
        "cannot apply cgroup limits ({}). \
         Run as root, enable cgroup delegation, or use --max-rss for best-effort memory polling instead",
        errors.join("; ")
    ))
}

fn setup_in_base(
    base: &std::path::Path,
    job_id: &str,
    pid: u32,
    limits: &Limits,
) -> Result<PathBuf> {
    let dir = base.join(job_id);
    // Remove a stale group from a previous incarnation (ignore failures:
    // a non-empty group simply gets reused).
    let _ = std::fs::remove_dir(&dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;

    // Controllers must be enabled at EVERY level: enabling them in our own
    // `bgrun/` dir (which we own) lets children use them. Best-effort —
    // the limit writes below surface real failures.
    let mut enable = String::new();
    if limits.memory_max.is_some() {
        enable.push_str("+memory ");
    }
    if limits.cpu_quota_pct.is_some() {
        enable.push_str("+cpu ");
    }
    if limits.pids_max.is_some() {
        enable.push_str("+pids ");
    }
    if !enable.is_empty() {
        let _ = std::fs::write(base.join("cgroup.subtree_control"), &enable);
    }

    if let Some(bytes) = limits.memory_max {
        std::fs::write(dir.join("memory.max"), bytes.to_string())
            .context("cannot set memory.max (controller not enabled?)")?;
    }
    if let Some(pct) = limits.cpu_quota_pct {
        std::fs::write(dir.join("cpu.max"), Limits::cpu_max_content(pct))
            .context("cannot set cpu.max (controller not enabled?)")?;
    }
    if let Some(max) = limits.pids_max {
        std::fs::write(dir.join("pids.max"), max.to_string())
            .context("cannot set pids.max (controller not enabled?)")?;
    }
    // cgroup.procs moves the whole thread group; future children inherit it.
    // Processes forked between spawn and this write stay outside (documented).
    std::fs::write(dir.join("cgroup.procs"), pid.to_string()).context("cannot add pid")?;
    Ok(dir)
}

/// Removes a job's group from every known base. Best-effort: succeeds once
/// the group's processes are gone (call after process exit).
pub fn remove_job_group(job_id: &str) {
    let uid = nix::unistd::getuid();
    for base in [
        PathBuf::from("/sys/fs/cgroup/bgrun"),
        PathBuf::from(format!(
            "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/bgrun"
        )),
    ] {
        let _ = std::fs::remove_dir(base.join(job_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_max_content_scales() {
        assert_eq!(Limits::cpu_max_content(50), "50000 100000");
        assert_eq!(Limits::cpu_max_content(100), "100000 100000");
        assert_eq!(Limits::cpu_max_content(200), "200000 100000");
    }

    #[test]
    fn limits_empty_detection() {
        assert!(Limits::default().is_empty());
        assert!(
            !Limits {
                memory_max: Some(1),
                ..Default::default()
            }
            .is_empty()
        );
        assert!(
            !Limits {
                cpu_quota_pct: Some(1),
                ..Default::default()
            }
            .is_empty()
        );
        assert!(
            !Limits {
                pids_max: Some(1),
                ..Default::default()
            }
            .is_empty()
        );
    }
}
