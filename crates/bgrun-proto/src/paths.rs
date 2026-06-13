use std::path::PathBuf;

/// Returns the state directory path using XDG data dir or fallback.
pub fn state_dir() -> PathBuf {
    if let Some(proj_dirs) = directories::ProjectDirs::from("", "", "bgrun") {
        proj_dirs.data_dir().to_path_buf()
    } else {
        // SAFETY: getuid(2) is a read-only system call that returns
        // the real user ID. It has no memory-safety invariants and is
        // always safe to call from any context.
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/bgrun-{}", uid))
    }
}

/// Returns the socket path from XDG_RUNTIME_DIR or fallback.
pub fn socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(runtime_dir).join("bgrun").join("daemon.sock")
    } else {
        // SAFETY: Same as above — getuid(2) is side-effect-free and
        // requires no special state or valid pointer arguments.
        let uid = unsafe { libc::getuid() };
        PathBuf::from(format!("/tmp/bgrun-{}", uid)).join("daemon.sock")
    }
}

/// Returns the directory for a specific job.
pub fn job_dir(id: &str) -> PathBuf {
    state_dir().join("jobs").join(id)
}
