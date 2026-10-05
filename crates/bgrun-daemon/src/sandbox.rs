//! Linux namespace sandbox for jobs.
//!
//! `sandbox_pre_exec` runs inside `Command::pre_exec`, i.e. after fork and
//! before exec in the child. It performs only syscalls and small file
//! writes: `unshare` plus the UID/GID map for unprivileged callers.

use std::io::Result as IoResult;

/// Content for `/proc/self/uid_map` / `gid_map`: maps our own outer id to
/// container root (0). A single self-mapping is permitted unprivileged.
///
/// Kept for callers outside `pre_exec` (and tests). The `pre_exec` path
/// below cannot use heap allocation (`format!`) after `fork` in a
/// multithreaded process (see `write_file_raw`).
pub fn id_map_content(outer_id: u32) -> String {
    let mut buf = [0u8; 32];
    let len = write_id_map(&mut buf, outer_id);
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

/// Renders `0 <outer_id> 1\n` into `buf` without heap allocation.
/// Returns the number of bytes written.
fn write_id_map(buf: &mut [u8; 32], outer_id: u32) -> usize {
    buf[0] = b'0';
    buf[1] = b' ';
    let mut digits = [0u8; 10];
    let mut n = 0usize;
    let mut v = outer_id;
    if v == 0 {
        digits[0] = b'0';
        n = 1;
    } else {
        while v > 0 {
            digits[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    let mut pos = 2usize;
    for i in (0..n).rev() {
        buf[pos] = digits[i];
        pos += 1;
    }
    buf[pos] = b' ';
    buf[pos + 1] = b'1';
    buf[pos + 2] = b'\n';
    pos + 3
}

/// Writes `bytes` to `path` using only raw syscalls on stack data.
///
/// `std::fs::write` must not be called inside `pre_exec`: after `fork` in a
/// multithreaded (Tokio) process only async-signal-safe functions are
/// allowed, and the std file APIs allocate and take internal locks — a
/// child can deadlock before `exec` if another thread held the allocator
/// lock at fork time.
///
/// The `write` return value is `isize`; a successful write is `>= 1` (the
/// `r < 0` branch returns first), so the `as usize` conversion cannot lose
/// the sign.
#[allow(clippy::cast_sign_loss)]
fn write_file_raw(path: &[u8], bytes: &[u8]) -> IoResult<()> {
    // NUL-terminate the path on the stack (longest path used here is
    // "/proc/self/setgroups", well under 64 bytes).
    let mut cpath = [0u8; 64];
    if path.len() + 1 > cpath.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "sandbox proc path too long",
        ));
    }
    cpath[..path.len()].copy_from_slice(path);
    cpath[path.len()] = 0;
    // SAFETY: `cpath` is a valid NUL-terminated stack buffer; flags/mode are
    // constants. `open`/`write`/`close` are async-signal-safe per POSIX.
    #[allow(unsafe_code)]
    unsafe {
        let fd = libc::open(cpath.as_ptr().cast(), libc::O_WRONLY | libc::O_CLOEXEC);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut written = 0usize;
        while written < bytes.len() {
            let r = libc::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written);
            if r < 0 {
                let e = std::io::Error::last_os_error();
                libc::close(fd);
                return Err(e);
            }
            written += r as usize;
        }
        libc::close(fd);
    }
    Ok(())
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
        // Raw syscalls on stack buffers only: this runs in `pre_exec`
        // (post-fork child), where `std::fs::write`/`format!` may deadlock.
        write_file_raw(b"/proc/self/setgroups", b"deny\n").map_err(std::io::Error::other)?;
        let mut uid_map = [0u8; 32];
        let uid_len = write_id_map(&mut uid_map, uid.as_raw());
        write_file_raw(b"/proc/self/uid_map", &uid_map[..uid_len])
            .map_err(std::io::Error::other)?;
        let mut gid_map = [0u8; 32];
        let gid_len = write_id_map(&mut gid_map, gid.as_raw());
        write_file_raw(b"/proc/self/gid_map", &gid_map[..gid_len])
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
