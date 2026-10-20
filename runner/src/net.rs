//! The "ampoule_os" import module: sockets, pipes and dup, as declared in ampoule_os.h (the spawn
//! driver's imports are in `spawn`).
//!
//! Guest constants come from two places: wasi-libc (AF_*, SOCK_*, MSG_PEEK/WAITALL, SHUT_*)
//! and ampoule_os.h, which fills the gaps with Linux numbering (SOL_SOCKET, SO_*, IPPROTO_*,
//! the other MSG_* flags). Each is translated to the host's own values here. Every call returns
//! a value >= 0 or a negated WASI errno.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd};
use std::sync::Arc;

use rustix::net::addr::SocketAddrArg;
use rustix::net::{AddressFamily, SocketAddrAny, SocketType};
use wasmtime::{Caller, Linker};

use crate::errno::{self, check};
use crate::kernel::{Entry, Kernel, Kind};
use crate::mem::R;
use crate::{k, unix, Ctx};

const G_AF_INET: u16 = 1;
const G_AF_INET6: u16 = 2;
const G_SOCK_DGRAM: i32 = 5;
const G_SOCK_STREAM: i32 = 6;
const G_SOCK_NONBLOCK: i32 = 0x4000;
const G_SOCK_CLOEXEC: i32 = 0x2000;

const G_SOL_SOCKET: i32 = 0x7fffffff;
const G_IPPROTO_IP: i32 = 0;
const G_IPPROTO_TCP: i32 = 6;
const G_IPPROTO_IPV6: i32 = 41;

const G_MSG_PEEK: i32 = 1;
const G_MSG_WAITALL: i32 = 2;
const G_MSG_DONTROUTE: i32 = 0x4;
const G_MSG_DONTWAIT: i32 = 0x40;
const G_MSG_OOB: i32 = 0x10000;

pub(crate) fn ret(r: R<i32>) -> i32 {
    match r {
        Ok(v) => v,
        Err(e) => -e,
    }
}

/// The socket at guest `fd`: its entry, which keeps the host fd open while held.
fn sock(k: &Kernel, fd: i32) -> R<Arc<Entry>> {
    let e = k.get(fd)?;
    if !e.is_socket() {
        return Err(errno::NOTSOCK);
    }
    Ok(e)
}

/// A host socket of type SOCK_DGRAM.
pub fn is_datagram(fd: BorrowedFd<'_>) -> bool {
    rustix::net::sockopt::socket_type(fd) == Ok(SocketType::DGRAM)
}

/// Decode a wasi-libc sockaddr (family u16 @0, port BE @2, v4 addr @4 / v6 flowinfo @4,
/// addr @8, scope @24).
fn read_addr(k: &Kernel, addr: u32, len: u32) -> R<SocketAddr> {
    let b = k.mem.read(addr, len.min(32))?;
    if b.len() < 2 {
        return Err(errno::INVAL);
    }
    let family = u16::from_le_bytes([b[0], b[1]]);
    match family {
        G_AF_INET if b.len() >= 8 => {
            let port = u16::from_be_bytes([b[2], b[3]]);
            Ok(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(b[4], b[5], b[6], b[7]), port)))
        },
        G_AF_INET6 if b.len() >= 28 => {
            let port = u16::from_be_bytes([b[2], b[3]]);
            let flow = u32::from_be_bytes(b[4..8].try_into().unwrap());
            let ip: [u8; 16] = b[8..24].try_into().unwrap();
            let scope = u32::from_le_bytes(b[24..28].try_into().unwrap());
            Ok(SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::from(ip), port, flow, scope)))
        },
        _ => Err(errno::AFNOSUPPORT),
    }
}

/// The address family of the guest sockaddr at `addr`.
fn family(k: &Kernel, addr: u32, len: u32) -> R<u16> {
    if len < 2 {
        return Err(errno::INVAL);
    }
    k.mem.u16(addr)
}

/// Report the peer address a host call filled in: an IP address, or an unnamed unix socket (the
/// host path of a named one means nothing to the guest). Other families are left unwritten.
fn write_peer(k: &Kernel, st: &libc::sockaddr_storage, addr: u32, len_ptr: u32) -> R<()> {
    if st.ss_family as i32 == libc::AF_UNIX {
        return unix::write_addr(k, None, addr, len_ptr);
    }
    match from_host(st) {
        Ok(sa) => write_addr(k, sa, addr, len_ptr),
        Err(_) => Ok(()),
    }
}

/// `write_peer` for an address rustix returned on `sock`. None is an unnamed unix peer (a client
/// that never bound), reported as such; other families give an address.
fn write_peer_any(k: &Kernel, sock: BorrowedFd<'_>, peer: Option<SocketAddrAny>, addr: u32, len_ptr: u32) -> R<()> {
    let family = match &peer {
        Some(a) => a.address_family(),
        None => rustix::net::getsockname(sock).map(|a| a.address_family()).unwrap_or(AddressFamily::UNSPEC),
    };
    if family == AddressFamily::UNIX {
        return unix::write_addr(k, None, addr, len_ptr);
    }
    match peer.map(SocketAddr::try_from) {
        Some(Ok(sa)) => write_addr(k, sa, addr, len_ptr),
        _ => Ok(()),
    }
}

/// Encode `sa` as a wasi-libc sockaddr at `addr`, honouring the guest's length in `len_ptr`.
fn write_addr(k: &Kernel, sa: SocketAddr, addr: u32, len_ptr: u32) -> R<()> {
    if addr == 0 || len_ptr == 0 {
        return Ok(());
    }
    let mut b = Vec::with_capacity(32);
    match sa {
        SocketAddr::V4(a) => {
            b.extend_from_slice(&G_AF_INET.to_le_bytes());
            b.extend_from_slice(&a.port().to_be_bytes());
            b.extend_from_slice(&a.ip().octets());
            b.resize(16, 0);
        },
        SocketAddr::V6(a) => {
            b.extend_from_slice(&G_AF_INET6.to_le_bytes());
            b.extend_from_slice(&a.port().to_be_bytes());
            b.extend_from_slice(&a.flowinfo().to_be_bytes());
            b.extend_from_slice(&a.ip().octets());
            b.extend_from_slice(&a.scope_id().to_le_bytes());
            b.resize(32, 0);
        },
    }
    let room = k.mem.u32(len_ptr)? as usize;
    k.mem.write(addr, &b[..b.len().min(room)])?;
    k.mem.set_u32(len_ptr, b.len() as u32)
}

/// A sockaddr the host filled in (`recvfrom`, which writes guest memory directly and so stays a raw
/// libc call) as an IP address.
fn from_host(st: &libc::sockaddr_storage) -> R<SocketAddr> {
    match st.ss_family as i32 {
        libc::AF_INET => {
            // SAFETY: ss_family says the storage holds a sockaddr_in, which it is large and aligned
            // enough for.
            let s = unsafe { &*(st as *const _ as *const libc::sockaddr_in) };
            let ip = Ipv4Addr::from(s.sin_addr.s_addr.to_ne_bytes());
            Ok(SocketAddr::new(IpAddr::V4(ip), u16::from_be(s.sin_port)))
        },
        libc::AF_INET6 => {
            // SAFETY: as above, for sockaddr_in6.
            let s = unsafe { &*(st as *const _ as *const libc::sockaddr_in6) };
            Ok(SocketAddr::V6(SocketAddrV6::new(
                Ipv6Addr::from(s.sin6_addr.s6_addr),
                u16::from_be(s.sin6_port),
                u32::from_be(s.sin6_flowinfo),
                s.sin6_scope_id,
            )))
        },
        _ => Err(errno::AFNOSUPPORT),
    }
}

/// Map a guest (level, name) to the host's; None means "accept and ignore".
fn sockopt(level: i32, name: i32) -> Option<(libc::c_int, libc::c_int)> {
    let host = match (level, name) {
        (G_SOL_SOCKET, 2) => (libc::SOL_SOCKET, libc::SO_REUSEADDR),
        (G_SOL_SOCKET, 3) => (libc::SOL_SOCKET, libc::SO_TYPE),
        (G_SOL_SOCKET, 4) => (libc::SOL_SOCKET, libc::SO_ERROR),
        (G_SOL_SOCKET, 5) => (libc::SOL_SOCKET, libc::SO_DONTROUTE),
        (G_SOL_SOCKET, 6) => (libc::SOL_SOCKET, libc::SO_BROADCAST),
        (G_SOL_SOCKET, 7) => (libc::SOL_SOCKET, libc::SO_SNDBUF),
        (G_SOL_SOCKET, 8) => (libc::SOL_SOCKET, libc::SO_RCVBUF),
        (G_SOL_SOCKET, 9) => (libc::SOL_SOCKET, libc::SO_KEEPALIVE),
        (G_SOL_SOCKET, 10) => (libc::SOL_SOCKET, libc::SO_OOBINLINE),
        (G_SOL_SOCKET, 13) => (libc::SOL_SOCKET, libc::SO_LINGER),
        (G_SOL_SOCKET, 15) => (libc::SOL_SOCKET, libc::SO_REUSEPORT),
        (G_SOL_SOCKET, 18) => (libc::SOL_SOCKET, libc::SO_RCVLOWAT),
        (G_SOL_SOCKET, 19) => (libc::SOL_SOCKET, libc::SO_SNDLOWAT),
        (G_SOL_SOCKET, 30) => (libc::SOL_SOCKET, libc::SO_ACCEPTCONN),
        (G_IPPROTO_TCP, 1) => (libc::IPPROTO_TCP, libc::TCP_NODELAY),
        (G_IPPROTO_IP, 1) => (libc::IPPROTO_IP, libc::IP_TOS),
        (G_IPPROTO_IP, 2) => (libc::IPPROTO_IP, libc::IP_TTL),
        (G_IPPROTO_IPV6, 26) => (libc::IPPROTO_IPV6, libc::IPV6_V6ONLY),
        _ => return None,
    };
    Some(host)
}

fn msg_flags(f: i32) -> libc::c_int {
    let mut h = 0;
    if f & G_MSG_PEEK != 0 {
        h |= libc::MSG_PEEK;
    }
    if f & G_MSG_WAITALL != 0 {
        h |= libc::MSG_WAITALL;
    }
    if f & G_MSG_DONTROUTE != 0 {
        h |= libc::MSG_DONTROUTE;
    }
    if f & G_MSG_DONTWAIT != 0 {
        h |= libc::MSG_DONTWAIT;
    }
    if f & G_MSG_OOB != 0 {
        h |= libc::MSG_OOB;
    }
    #[cfg(target_os = "linux")]
    {
        h |= libc::MSG_NOSIGNAL;
    }
    h
}

/// wasi-libc's SHUT_* (WASI's sdflags: RD = 1, WR = 2) as the host's.
pub fn shutdown_how(how: i32) -> R<rustix::net::Shutdown> {
    match how {
        1 => Ok(rustix::net::Shutdown::Read),
        2 => Ok(rustix::net::Shutdown::Write),
        3 => Ok(rustix::net::Shutdown::Both),
        _ => Err(errno::INVAL),
    }
}

/// Install a new host socket (close-on-exec already, see `fds`) in the guest's fd table: optionally
/// non-blocking, and (on macOS, which has no MSG_NOSIGNAL) never raising SIGPIPE.
pub fn adopt_socket(k: &Kernel, owned: OwnedFd, nonblock: bool) -> R<i32> {
    if nonblock {
        if let Ok(fl) = rustix::fs::fcntl_getfl(&owned) {
            let _ = rustix::fs::fcntl_setfl(&owned, fl | rustix::fs::OFlags::NONBLOCK);
        }
    }
    #[cfg(target_os = "macos")]
    let _ = rustix::net::sockopt::set_socket_nosigpipe(&owned, true);
    k.insert(Kind::Socket(owned))
}

pub fn add(linker: &mut Linker<Ctx>) -> anyhow::Result<()> {
    let m = "ampoule_os";

    linker.func_wrap(m, "socket", |c: Caller<'_, Ctx>, domain: i32, ty: i32, proto: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let d = match domain as u16 {
                G_AF_INET => libc::AF_INET,
                G_AF_INET6 => libc::AF_INET6,
                unix::G_AF_UNIX => libc::AF_UNIX,
                _ => return Err(errno::AFNOSUPPORT),
            };
            let t = match ty & !(G_SOCK_NONBLOCK | G_SOCK_CLOEXEC) {
                G_SOCK_STREAM => libc::SOCK_STREAM,
                G_SOCK_DGRAM => libc::SOCK_DGRAM,
                _ => return Err(errno::PROTONOSUPPORT),
            };
            let p = match proto {
                0 => 0,
                6 => libc::IPPROTO_TCP,
                17 => libc::IPPROTO_UDP,
                _ => return Err(errno::PROTONOSUPPORT),
            };
            adopt_socket(k, crate::fds::socket(d, t, p)?, ty & G_SOCK_NONBLOCK != 0)
        })())
    })?;

    linker.func_wrap(m, "bind", |c: Caller<'_, Ctx>, fd: i32, addr: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let e = sock(k, fd)?;
            let s = e.fd()?;
            if family(k, addr as u32, len as u32)? == unix::G_AF_UNIX {
                let path = unix::read_path(k, addr as u32, len as u32)?.ok_or(errno::INVAL)?;
                return unix::bind(k, s, &path).map(|()| 0);
            }
            let guest = read_addr(k, addr as u32, len as u32)?;
            let sa = k.policy.bind(guest)?;
            // With --idle-stop, a port the supervisor holds gets its listener instead of a new bind.
            if let Some(r) = crate::idle::take_over(guest.port(), s.as_raw_fd()) {
                return r.map(|()| 0);
            }
            rustix::net::bind(s, &sa).map_err(errno::rx)?;
            crate::idle::bound(s.as_raw_fd(), guest.port());
            Ok(0)
        })())
    })?;

    linker.func_wrap(m, "listen", |c: Caller<'_, Ctx>, fd: i32, backlog: i32| -> i32 {
        let k = k(&c);
        ret(sock(k, fd).and_then(|e| {
            let s = e.fd()?;
            rustix::net::listen(s, backlog).map_err(errno::rx)?;
            crate::idle::listening(s);
            Ok(0)
        }))
    })?;

    linker.func_wrap(m, "accept", |c: Caller<'_, Ctx>, fd: i32, addr: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let e = sock(k, fd)?;
            let s = e.fd()?;
            let (conn, peer) = crate::fds::accept(s, true)?;
            let nfd = conn.as_raw_fd();
            let gfd = adopt_socket(k, conn, false)?;
            crate::idle::accepted(s.as_raw_fd(), nfd);
            if let Err(e) = write_peer_any(k, s, peer, addr as u32, len as u32) {
                // The guest never learns about gfd, so it must not stay in the table.
                drop(k.remove(gfd));
                return Err(e);
            }
            Ok(gfd)
        })())
    })?;

    linker.func_wrap(m, "connect", |c: Caller<'_, Ctx>, fd: i32, addr: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let e = sock(k, fd)?;
            let s = e.fd()?;
            if family(k, addr as u32, len as u32)? == unix::G_AF_UNIX {
                let path = unix::read_path(k, addr as u32, len as u32)?.ok_or(errno::INVAL)?;
                return unix::connect(k, s, &path).map(|()| 0);
            }
            let sa = k.policy.connect(read_addr(k, addr as u32, len as u32)?)?;
            rustix::net::connect(s, &sa).map(|()| 0).map_err(errno::rx)
        })())
    })?;

    linker.func_wrap(m, "getsockopt", |c: Caller<'_, Ctx>, fd: i32, level: i32, name: i32, val: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let e = sock(k, fd)?;
            let raw = e.fd()?.as_raw_fd();
            let room = k.mem.u32(len as u32)?;
            let Some((hl, hn)) = sockopt(level, name) else {
                k.mem.write(val as u32, &vec![0u8; room.min(16) as usize])?;
                return Ok(0);
            };
            let mut buf = [0u8; 16];
            let mut l = room.min(16) as libc::socklen_t;
            // SAFETY: a 16-byte host buffer and its length; `e` keeps `raw` open. Raw libc, as the
            // option is any of those `sockopt` maps, with the guest's length.
            check(unsafe { libc::getsockopt(raw, hl, hn, buf.as_mut_ptr() as *mut libc::c_void, &mut l) })?;
            // Values with host-specific numbering, in the guest's terms.
            let host_int = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
            let guest_int = match (hl, hn) {
                (libc::SOL_SOCKET, libc::SO_ERROR) => Some(errno::from_host(host_int)),
                (libc::SOL_SOCKET, libc::SO_TYPE) => Some(match host_int {
                    libc::SOCK_STREAM => G_SOCK_STREAM,
                    libc::SOCK_DGRAM => G_SOCK_DGRAM,
                    other => other,
                }),
                _ => None,
            };
            if let Some(v) = guest_int {
                buf[..4].copy_from_slice(&v.to_le_bytes());
            }
            k.mem.write(val as u32, &buf[..l as usize])?;
            k.mem.set_u32(len as u32, l as u32)?;
            Ok(0)
        })())
    })?;

    linker.func_wrap(m, "setsockopt", |c: Caller<'_, Ctx>, fd: i32, level: i32, name: i32, val: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let e = sock(k, fd)?;
            let raw = e.fd()?.as_raw_fd();
            let Some((hl, hn)) = sockopt(level, name) else { return Ok(0) };
            let v = k.mem.read(val as u32, (len as u32).min(16))?;
            // SAFETY: a host copy of the guest's value and its length; `e` keeps `raw` open.
            check(unsafe { libc::setsockopt(raw, hl, hn, v.as_ptr() as *const libc::c_void, v.len() as libc::socklen_t) })
        })())
    })?;

    linker.func_wrap(m, "getsockname", |c: Caller<'_, Ctx>, fd: i32, addr: i32, len: i32| -> i32 {
        let k = k(&c);
        ret(name_of(k, fd, addr as u32, len as u32, false))
    })?;

    linker.func_wrap(m, "getpeername", |c: Caller<'_, Ctx>, fd: i32, addr: i32, len: i32| -> i32 {
        let k = k(&c);
        ret(name_of(k, fd, addr as u32, len as u32, true))
    })?;

    linker.func_wrap(m, "recvfrom", |c: Caller<'_, Ctx>, fd: i32, buf: i32, len: i32, flags: i32, addr: i32, alen: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let e = sock(k, fd)?;
            let raw = e.fd()?.as_raw_fd();
            let p = k.mem.ptr(buf as u32, len as u32)?;
            // SAFETY: all-zero is a valid sockaddr_storage.
            let mut st: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut l = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            // SAFETY: `p` is `len` bounds-checked guest bytes, written by the kernel straight into
            // guest memory (no Rust slice over it, see `mem`); `e` keeps `raw` open.
            let r =
                unsafe { libc::recvfrom(raw, p as *mut libc::c_void, len as u32 as usize, msg_flags(flags), &mut st as *mut _ as *mut libc::sockaddr, &mut l) };
            if r < 0 {
                return Err(errno::last());
            }
            if addr != 0 && l > 0 {
                write_peer(k, &st, addr as u32, alen as u32)?;
            }
            Ok(r as i32)
        })())
    })?;

    linker.func_wrap(m, "sendto", |c: Caller<'_, Ctx>, fd: i32, buf: i32, len: i32, flags: i32, addr: i32, alen: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let e = sock(k, fd)?;
            let raw = e.fd()?.as_raw_fd();
            let p = k.mem.ptr(buf as u32, len as u32)? as *const libc::c_void;
            let n = len as u32 as usize;
            // The data is guest memory, read by the kernel through `p` (no Rust slice over it, see
            // `mem`), so these stay raw libc calls; `e` keeps `raw` open throughout.
            // SAFETY (each call below): `p` is `n` bounds-checked guest bytes the kernel reads in
            // place; an address comes from rustix's `with_sockaddr`, valid for the closure it calls.
            let send_to = |a: *const libc::sockaddr, l| unsafe { libc::sendto(raw, p, n, msg_flags(flags), a, l) };
            let r = if addr == 0 {
                unsafe { libc::send(raw, p, n, msg_flags(flags)) }
            } else if family(k, addr as u32, alen as u32)? == unix::G_AF_UNIX {
                let path = unix::read_path(k, addr as u32, alen as u32)?.ok_or(errno::INVAL)?;
                let t = unix::target(k, &path)?;
                unsafe { t.addr.with_sockaddr(|a, l| send_to(a.cast(), l as libc::socklen_t)) }
            } else {
                let sa = k.policy.connect(read_addr(k, addr as u32, alen as u32)?)?;
                unsafe { sa.with_sockaddr(|a, l| send_to(a.cast(), l as libc::socklen_t)) }
            };
            if r < 0 {
                return Err(errno::last());
            }
            Ok(r as i32)
        })())
    })?;

    linker.func_wrap(m, "shutdown", |c: Caller<'_, Ctx>, fd: i32, how: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let e = sock(k, fd)?;
            rustix::net::shutdown(e.fd()?, shutdown_how(how)?).map(|()| 0).map_err(errno::rx)
        })())
    })?;

    linker.func_wrap(m, "pipe", |c: Caller<'_, Ctx>, fds: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            k.mem.ptr(fds as u32, 8)?;
            // Owned at once, so both ends close on any error below.
            let (rd, wr) = crate::fds::pipe()?;
            let r = k.insert(Kind::Pipe(rd))?;
            let w = match k.insert(Kind::Pipe(wr)) {
                Ok(w) => w,
                Err(e) => {
                    drop(k.remove(r));
                    return Err(e);
                },
            };
            k.mem.set_u32(fds as u32, r as u32)?;
            k.mem.set_u32(fds as u32 + 4, w as u32)?;
            Ok(0)
        })())
    })?;

    linker.func_wrap(m, "dup", |c: Caller<'_, Ctx>, fd: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let e = k.get(fd)?;
            let clone = |fd: &OwnedFd| fd.try_clone().map_err(|e| errno::of_io(&e));
            let kind = match &e.kind {
                Kind::Stdio(raw) => Kind::Stdio(*raw),
                Kind::File { fd, ro } => Kind::File { fd: clone(fd)?, ro: *ro },
                Kind::Socket(fd) => Kind::Socket(clone(fd)?),
                Kind::Pipe(fd) => Kind::Pipe(clone(fd)?),
                // No host fd to duplicate: the new number shares the entry, and its position.
                Kind::Mem { .. } | Kind::Dir { .. } => return k.insert_entry(e.clone()),
            };
            k.insert(kind)
        })())
    })?;

    // The guest's getpid is this process's: the pid `ps` lists and `kill` reaches. Under the
    // --restart/--idle-stop supervisor, that is the child running the VM.
    linker.func_wrap(m, "getpid", |_c: Caller<'_, Ctx>| -> i32 { std::process::id() as i32 })?;

    linker.func_wrap(m, "gethostname", |c: Caller<'_, Ctx>, name: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            // The node name uname reports is the host name gethostname reads.
            let uts = rustix::system::uname();
            let host = uts.nodename().to_bytes_with_nul();
            if host.len() > len as usize {
                return Err(errno::NAMETOOLONG);
            }
            k.mem.write(name as u32, host)?;
            Ok(0)
        })())
    })?;

    linker.func_wrap(m, "signal_fd", |c: Caller<'_, Ctx>, fd: i32| {
        let k = k(&c);
        if let Ok(e) = k.get(fd) {
            let _ = k.signal_pipe.set(e);
        }
    })?;

    linker.func_wrap(m, "thread_exit", |_c: Caller<'_, Ctx>| -> wasmtime::Result<()> { Err(wasmtime::Error::new(crate::threads::ThreadExit)) })?;

    Ok(())
}

fn name_of(k: &Kernel, fd: i32, addr: u32, len: u32, peer: bool) -> R<i32> {
    let e = sock(k, fd)?;
    let s = e.fd()?;
    let host = if peer { rustix::net::getpeername(s) } else { rustix::net::getsockname(s).map(Some) };
    // An unnamed peer (a unix client that never bound) comes back as no address at all.
    let family = match &host {
        Ok(Some(a)) => a.address_family(),
        Ok(None) => rustix::net::getsockname(s).map_err(errno::rx)?.address_family(),
        Err(e) => return Err(errno::rx(*e)),
    };
    if family == AddressFamily::UNIX {
        unix::write_addr(k, unix::name(s.as_raw_fd(), peer).as_deref(), addr, len)?;
        return Ok(0);
    }
    let host = host.map_err(errno::rx)?.ok_or(errno::AFNOSUPPORT)?;
    let sa = SocketAddr::try_from(host).map_err(|_| errno::AFNOSUPPORT)?;
    let sa = if peer { sa } else { k.policy.guest_local(sa) };
    write_addr(k, sa, addr, len)?;
    Ok(0)
}
