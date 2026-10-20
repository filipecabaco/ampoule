//! Unix domain sockets (`{:local, Path}` in gen_tcp and gen_udp) at guest paths.
//!
//! A socket's path is a guest path, resolved through the mounts like any file: binding creates the
//! socket file in a host directory the guest may write to, connecting reaches a socket file the
//! guest can see. Nothing outside the mounts is reachable, and Linux's abstract namespace, which
//! is outside every mount, is refused.
//!
//! The host calls start from the directory the resolver opened, not from a path, so a symlink the
//! guest swaps in meanwhile cannot redirect them. On Linux, bind goes through
//! /proc/self/fd/<dir>/<name> and connect through an O_PATH fd of the socket file itself. macOS has
//! neither: there the socket file is checked and then reached by its directory's canonical path,
//! and a swap in between, inside a mount the guest can write, could still redirect a connect.

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::sync::Mutex;

#[cfg(not(target_os = "linux"))]
use rustix::fs::AtFlags;
#[cfg(target_os = "linux")]
use rustix::fs::{Mode, OFlags};
use rustix::net::SocketAddrUnix;

use crate::errno;
use crate::hostfs::HostNode;
use crate::kernel::{normalize, Kernel, Node};
use crate::mem::R;

/// wasi-libc's AF_UNIX.
pub const G_AF_UNIX: u16 = 3;
/// sizeof(sockaddr_un) in the guest: the family, then a 108-byte path (ampoule_os.h).
const GUEST_LEN: u32 = 110;

/// Guest paths of the unix sockets the guest bound or connected, by host fd. The host's own names
/// are host paths (or /proc/self/fd/...), which mean nothing to the guest.
static NAMES: Mutex<BTreeMap<RawFd, Names>> = Mutex::new(BTreeMap::new());

#[derive(Default)]
struct Names {
    local: Option<String>,
    peer: Option<String>,
}

fn names() -> std::sync::MutexGuard<'static, BTreeMap<RawFd, Names>> {
    NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The socket at `raw` is closing: forget its names before the number is reused.
pub fn closed(raw: RawFd) {
    names().remove(&raw);
}

/// The guest path in the sockaddr_un at `addr`, or None for an unnamed address.
pub fn read_path(k: &Kernel, addr: u32, len: u32) -> R<Option<String>> {
    let b = k.mem.read(addr, len.min(GUEST_LEN))?;
    let Some(path) = b.get(2..).filter(|p| !p.is_empty()) else { return Ok(None) };
    if path[0] == 0 {
        // Linux's abstract namespace: a host-wide name outside every mount.
        return Err(errno::ACCES);
    }
    let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
    let path = std::str::from_utf8(&path[..end]).map_err(|_| errno::INVAL)?;
    if !path.starts_with('/') {
        // ampoule_os.h makes relative paths absolute against the guest's cwd.
        return Err(errno::INVAL);
    }
    Ok(Some(normalize(path)))
}

/// Write a guest sockaddr_un for `path` (None: unnamed) at `addr`, within the guest's `len_ptr`.
pub fn write_addr(k: &Kernel, path: Option<&str>, addr: u32, len_ptr: u32) -> R<()> {
    if addr == 0 || len_ptr == 0 {
        return Ok(());
    }
    let mut b = G_AF_UNIX.to_le_bytes().to_vec();
    if let Some(p) = path {
        b.extend_from_slice(p.as_bytes());
        b.push(0);
    }
    let room = k.mem.u32(len_ptr)? as usize;
    k.mem.write(addr, &b[..b.len().min(room)])?;
    k.mem.set_u32(len_ptr, b.len() as u32)
}

/// The guest's name for a unix socket's own end (`peer`: the other end), when it has one.
pub fn name(raw: RawFd, peer: bool) -> Option<String> {
    let names = names();
    let n = names.get(&raw)?;
    if peer {
        n.peer.clone()
    } else {
        n.local.clone()
    }
}

fn host_addr(path: &[u8]) -> R<SocketAddrUnix> {
    SocketAddrUnix::new(std::ffi::OsStr::from_bytes(path)).map_err(errno::rx)
}

/// A path to `name` in the open directory `dir` that no later change to the tree can redirect
/// (Linux), or the directory's canonical path joined with `name` (macOS).
fn in_dir(dir: &OwnedFd, name: &CStr) -> R<Vec<u8>> {
    #[cfg(target_os = "linux")]
    let mut p = format!("/proc/self/fd/{}/", dir.as_raw_fd()).into_bytes();
    #[cfg(not(target_os = "linux"))]
    let mut p = {
        let mut p = rustix::fs::getpath(dir).map_err(errno::rx)?.into_bytes();
        p.push(b'/');
        p
    };
    p.extend_from_slice(name.to_bytes());
    Ok(p)
}

/// Bind the host socket `sock` at the guest path `guest`.
pub fn bind(k: &Kernel, sock: BorrowedFd<'_>, guest: &str) -> R<()> {
    let n = crate::wasi::host_for_write(k, guest, false, false)?;
    rustix::net::bind(sock, &host_addr(&in_dir(&n.dir, &n.name)?)?).map_err(errno::rx)?;
    names().entry(sock.as_raw_fd()).or_default().local = Some(guest.to_string());
    Ok(())
}

/// A host address reaching the socket file at the guest path `guest`, and what keeps that address
/// valid while it is used.
pub struct Target {
    pub addr: SocketAddrUnix,
    _held: Option<OwnedFd>,
}

pub fn target(k: &Kernel, guest: &str) -> R<Target> {
    let Node::Host(n) = k.resolve(guest, true)? else { return Err(errno::CONNREFUSED) };
    let HostNode { dir, name, mount_root, .. } = &n;
    // Below a mount point the resolver has followed every symlink the guest asked to; a mount
    // point itself is a path the runtime was given, followed as is.
    #[cfg(target_os = "linux")]
    {
        let nofollow = if *mount_root { OFlags::empty() } else { OFlags::NOFOLLOW };
        let fd = rustix::fs::openat(dir, name.as_c_str(), OFlags::PATH | OFlags::CLOEXEC | nofollow, Mode::empty()).map_err(errno::rx)?;
        is_socket(rustix::fs::fstat(&fd).map_err(errno::rx)?)?;
        let addr = host_addr(format!("/proc/self/fd/{}", fd.as_raw_fd()).as_bytes())?;
        Ok(Target { addr, _held: Some(fd) })
    }
    #[cfg(not(target_os = "linux"))]
    {
        let flags = if *mount_root { AtFlags::empty() } else { AtFlags::SYMLINK_NOFOLLOW };
        is_socket(rustix::fs::statat(dir, name.as_c_str(), flags).map_err(errno::rx)?)?;
        Ok(Target { addr: host_addr(&in_dir(dir, name)?)?, _held: None })
    }
}

fn is_socket(st: rustix::fs::Stat) -> R<()> {
    if rustix::fs::FileType::from_raw_mode(st.st_mode as _) == rustix::fs::FileType::Socket {
        Ok(())
    } else {
        Err(errno::CONNREFUSED)
    }
}

/// Connect the host socket `sock` to the guest path `guest`.
pub fn connect(k: &Kernel, sock: BorrowedFd<'_>, guest: &str) -> R<()> {
    let t = target(k, guest)?;
    rustix::net::connect(sock, &t.addr).map_err(errno::rx)?;
    names().entry(sock.as_raw_fd()).or_default().peer = Some(guest.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_addr_refuses_paths_longer_than_sun_path() {
        assert_eq!(host_addr(&[b'a'; 200]).err(), Some(errno::NAMETOOLONG));
        assert!(host_addr(b"/tmp/x.sock").is_ok());
    }
}
