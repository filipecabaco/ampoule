//! Host fds the runtime creates for the guest, close-on-exec from the moment they exist, so a
//! program the guest starts (see `spawn`) inherits only the fds meant for it, never another port's
//! pipe or a listening socket.
//!
//! Linux sets the flag as part of creating the fd (pipe2, SOCK_CLOEXEC, accept4, dup3). macOS has
//! no such calls: there, creating an fd and then setting the flag happen under a shared lock, and
//! `spawn` takes the lock exclusively around `fork` alone.

#[cfg(not(target_os = "linux"))]
use std::os::fd::{AsFd, BorrowedFd};
use std::os::fd::{OwnedFd, RawFd};
#[cfg(not(target_os = "linux"))]
use std::sync::RwLockReadGuard;
use std::sync::{PoisonError, RwLock, RwLockWriteGuard};

use rustix::net::{AddressFamily, Protocol, SocketAddrAny, SocketType};

use crate::errno::{self, check};
use crate::mem::R;

static FORK_LOCK: RwLock<()> = RwLock::new(());

/// Held while this thread makes an fd that is not close-on-exec yet (macOS only).
#[cfg(not(target_os = "linux"))]
fn creating() -> RwLockReadGuard<'static, ()> {
    FORK_LOCK.read().unwrap_or_else(PoisonError::into_inner)
}

/// Held across `fork`: no fd is half-made while the child copies the fd table.
pub fn forking() -> RwLockWriteGuard<'static, ()> {
    FORK_LOCK.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(not(target_os = "linux"))]
fn set_cloexec(fd: BorrowedFd<'_>) {
    let _ = rustix::io::fcntl_setfd(fd, rustix::io::FdFlags::CLOEXEC);
}

/// A pipe: (read end, write end).
pub fn pipe() -> R<(OwnedFd, OwnedFd)> {
    #[cfg(target_os = "linux")]
    return rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).map_err(errno::rx);
    #[cfg(not(target_os = "linux"))]
    {
        let _creating = creating();
        let (r, w) = rustix::pipe::pipe().map_err(errno::rx)?;
        set_cloexec(r.as_fd());
        set_cloexec(w.as_fd());
        Ok((r, w))
    }
}

/// A socket of the host's `domain`, `ty` and `proto` (0: the default for the type).
pub fn socket(domain: libc::c_int, ty: libc::c_int, proto: libc::c_int) -> R<OwnedFd> {
    let family = AddressFamily::from_raw(domain as _);
    let ty = SocketType::from_raw(ty as _);
    let proto = std::num::NonZeroU32::new(proto as u32).map(Protocol::from_raw);
    #[cfg(target_os = "linux")]
    return rustix::net::socket_with(family, ty, rustix::net::SocketFlags::CLOEXEC, proto).map_err(errno::rx);
    #[cfg(not(target_os = "linux"))]
    {
        let _creating = creating();
        let fd = rustix::net::socket(family, ty, proto).map_err(errno::rx)?;
        set_cloexec(fd.as_fd());
        Ok(fd)
    }
}

/// Accept a connection on `listener`, with the peer's address when `peer` asks for it.
pub fn accept(listener: impl std::os::fd::AsFd, peer: bool) -> R<(OwnedFd, Option<SocketAddrAny>)> {
    #[cfg(target_os = "linux")]
    {
        let flags = rustix::net::SocketFlags::CLOEXEC;
        if peer {
            rustix::net::acceptfrom_with(listener, flags).map_err(errno::rx)
        } else {
            rustix::net::accept_with(listener, flags).map(|fd| (fd, None)).map_err(errno::rx)
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        // A blocking accept cannot take the lock: it would hold up every fork until a client
        // arrives. It goes without, leaving a fork in the instant before the flag is set able to
        // copy the connection. ERTS's sockets are all non-blocking and never take this path.
        let listener = listener.as_fd();
        let blocking = !rustix::fs::fcntl_getfl(listener).map_err(errno::rx)?.contains(rustix::fs::OFlags::NONBLOCK);
        let _creating = (!blocking).then(creating);
        let accepted = if peer { rustix::net::acceptfrom(listener) } else { rustix::net::accept(listener).map(|fd| (fd, None)) };
        let (fd, addr) = accepted.map_err(errno::rx)?;
        set_cloexec(fd.as_fd());
        Ok((fd, addr))
    }
}

/// dup2(`from`, `to`), leaving `to` close-on-exec. Raw libc: `to` stays owned by the guest's fd
/// table, which rustix's dup2 (wanting `&mut OwnedFd`) cannot express.
pub fn dup_onto(from: RawFd, to: RawFd) -> R<()> {
    #[cfg(target_os = "linux")]
    if from != to {
        // SAFETY: plain fd numbers; `to` is replaced in place and stays owned by its entry.
        check(unsafe { libc::dup3(from, to, libc::O_CLOEXEC) })?;
        return Ok(());
    }
    #[cfg(not(target_os = "linux"))]
    let _creating = creating();
    // SAFETY: as above; FD_CLOEXEC on an fd this process holds.
    check(unsafe { libc::dup2(from, to) })?;
    // SAFETY: as above.
    unsafe { libc::fcntl(to, libc::F_SETFD, libc::FD_CLOEXEC) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::fd::{AsFd, AsRawFd};

    use super::*;

    fn is_cloexec(fd: &OwnedFd) -> bool {
        rustix::io::fcntl_getfd(fd.as_fd()).unwrap().contains(rustix::io::FdFlags::CLOEXEC)
    }

    #[test]
    fn pipes_and_sockets_are_close_on_exec() {
        let (r, w) = pipe().unwrap();
        let s = socket(libc::AF_INET, libc::SOCK_STREAM, 0).unwrap();
        assert!(is_cloexec(&r) && is_cloexec(&w) && is_cloexec(&s));
    }

    #[test]
    fn dup_onto_leaves_the_target_close_on_exec() {
        let (r, w) = pipe().unwrap();
        dup_onto(r.as_raw_fd(), w.as_raw_fd()).unwrap();
        assert!(is_cloexec(&w));
    }
}
