//! Linux namespace sandbox for jobs.
//!
//! `sandbox_pre_exec` runs inside `Command::pre_exec`, i.e. after fork and
//! before exec in the child. It performs only syscalls and small file
//! writes: `unshare` plus the UID/GID map for unprivileged callers.

use std::io::Result as IoResult;

/// Content for `/proc/self/uid_map` / `gid_map`: maps our own outer id to
/// container root (0). A single self-mapping is permitted unprivileged.
pub fn id_map_content(outer_id: u32) -> String {
    format!("0 {outer_id} 1\n")
}

/// Enters sandbox namespaces: new network + IPC namespaces in all cases,
/// plus a user namespace with a self UID/GID map for unprivileged callers
/// (unprivileged `unshare(CLONE_NEWNET)` would fail with EPERM otherwise).
/// Root keeps its identity (no user namespace); sandboxed root can still
/// only see loopback and its own IPC objects.
pub fn sandbox_pre_exec(uid: nix::unistd::Uid, gid: nix::unistd::Gid) -> IoResult<()> {
    use nix::sched::{CloneFlags, unshare};
    if uid.is_root() {
        unshare(CloneFlags::CLONE_NEWNET | CloneFlags::CLONE_NEWIPC)
            .map_err(std::io::Error::other)?;
    } else {
        unshare(CloneFlags::CLONE_NEWUSER | CloneFlags::CLONE_NEWNET | CloneFlags::CLONE_NEWIPC)
            .map_err(std::io::Error::other)?;
        // Since 3.19 the GID map requires setgroups to be denied first.
        std::fs::write("/proc/self/setgroups", "deny\n").map_err(std::io::Error::other)?;
        std::fs::write("/proc/self/uid_map", id_map_content(uid.as_raw()))
            .map_err(std::io::Error::other)?;
        std::fs::write("/proc/self/gid_map", id_map_content(gid.as_raw()))
            .map_err(std::io::Error::other)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_map_content_maps_self_to_root() {
        assert_eq!(id_map_content(1000), "0 1000 1\n");
        assert_eq!(id_map_content(0), "0 0 1\n");
    }
}
