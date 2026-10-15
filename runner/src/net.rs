//! The "ampoule_os" import module: sockets and pipes, as declared in ampoule_os.h.
//!
//! Guest constants come from two places: wasi-libc (AF_*, SOCK_*, MSG_PEEK/WAITALL, SHUT_*)
//! and ampoule_os.h, which fills the gaps with Linux numbering (SOL_SOCKET, SO_*, IPPROTO_*,
//! the other MSG_* flags). Each is translated to the host's own values here. Every call returns
//! a value >= 0 or a negated WASI errno.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::Arc;

use wasmtime::{Caller, Linker};

use crate::errno::{self, check};
use crate::kernel::{Entry, Kernel, Kind};
use crate::mem::R;
use crate::{k, Ctx};

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

fn ret(r: R<i32>) -> i32 {
    match r {
        Ok(v) => v,
        Err(e) => -e,
    }
}

/// The socket at guest `fd`: its entry, which keeps the host fd open while held, and the fd.
fn sock_fd(k: &Kernel, fd: i32) -> R<(Arc<Entry>, i32)> {
    let e = k.get(fd)?;
    if !e.is_socket() {
        return Err(errno::NOTSOCK);
    }
    let raw = e.host_fd()?;
    Ok((e, raw))
}

/// A host socket of type SOCK_DGRAM.
pub fn is_datagram(raw: libc::c_int) -> bool {
    let mut ty: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    let r = unsafe { libc::getsockopt(raw, libc::SOL_SOCKET, libc::SO_TYPE, (&mut ty as *mut libc::c_int).cast(), &mut len) };
    r == 0 && ty == libc::SOCK_DGRAM
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

fn to_host(sa: SocketAddr) -> (libc::sockaddr_storage, libc::socklen_t) {
    let mut st: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let len = match sa {
        SocketAddr::V4(a) => {
            let s = unsafe { &mut *(&mut st as *mut _ as *mut libc::sockaddr_in) };
            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            {
                s.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
            }
            s.sin_family = libc::AF_INET as libc::sa_family_t;
            s.sin_port = a.port().to_be();
            s.sin_addr = libc::in_addr { s_addr: u32::from_ne_bytes(a.ip().octets()) };
            std::mem::size_of::<libc::sockaddr_in>()
        },
        SocketAddr::V6(a) => {
            let s = unsafe { &mut *(&mut st as *mut _ as *mut libc::sockaddr_in6) };
            #[cfg(any(target_os = "macos", target_os = "freebsd"))]
            {
                s.sin6_len = std::mem::size_of::<libc::sockaddr_in6>() as u8;
            }
            s.sin6_family = libc::AF_INET6 as libc::sa_family_t;
            s.sin6_port = a.port().to_be();
            s.sin6_flowinfo = a.flowinfo().to_be();
            s.sin6_addr = libc::in6_addr { s6_addr: a.ip().octets() };
            s.sin6_scope_id = a.scope_id();
            std::mem::size_of::<libc::sockaddr_in6>()
        },
    };
    (st, len as libc::socklen_t)
}

fn from_host(st: &libc::sockaddr_storage) -> R<SocketAddr> {
    match st.ss_family as i32 {
        libc::AF_INET => {
            let s = unsafe { &*(st as *const _ as *const libc::sockaddr_in) };
            let ip = Ipv4Addr::from(s.sin_addr.s_addr.to_ne_bytes());
            Ok(SocketAddr::new(IpAddr::V4(ip), u16::from_be(s.sin_port)))
        },
        libc::AF_INET6 => {
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
pub fn shutdown_how(how: i32) -> R<libc::c_int> {
    match how {
        1 => Ok(libc::SHUT_RD),
        2 => Ok(libc::SHUT_WR),
        3 => Ok(libc::SHUT_RDWR),
        _ => Err(errno::INVAL),
    }
}

/// Install a new host socket in the guest's fd table: close-on-exec, optionally non-blocking,
/// and (on macOS, which has no MSG_NOSIGNAL) never raising SIGPIPE.
pub fn adopt_socket(k: &Kernel, raw: libc::c_int, nonblock: bool) -> R<i32> {
    let owned = unsafe { OwnedFd::from_raw_fd(raw) };
    unsafe {
        libc::fcntl(raw, libc::F_SETFD, libc::FD_CLOEXEC);
        if nonblock {
            let fl = libc::fcntl(raw, libc::F_GETFL);
            libc::fcntl(raw, libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
        #[cfg(target_os = "macos")]
        {
            let one: libc::c_int = 1;
            libc::setsockopt(raw, libc::SOL_SOCKET, libc::SO_NOSIGPIPE, &one as *const _ as *const libc::c_void, 4);
        }
    }
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
            let raw = check(unsafe { libc::socket(d, t, p) })?;
            adopt_socket(k, raw, ty & G_SOCK_NONBLOCK != 0)
        })())
    })?;

    linker.func_wrap(m, "bind", |c: Caller<'_, Ctx>, fd: i32, addr: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let (_held, raw) = sock_fd(k, fd)?;
            let guest = read_addr(k, addr as u32, len as u32)?;
            let sa = k.policy.bind(guest)?;
            // With --idle-stop, a port the supervisor holds gets its listener instead of a new bind.
            if let Some(r) = crate::idle::take_over(guest.port(), raw) {
                return r.map(|()| 0);
            }
            let (st, l) = to_host(sa);
            check(unsafe { libc::bind(raw, &st as *const _ as *const libc::sockaddr, l) })?;
            crate::idle::bound(raw, guest.port());
            Ok(0)
        })())
    })?;

    linker.func_wrap(m, "listen", |c: Caller<'_, Ctx>, fd: i32, backlog: i32| -> i32 {
        let k = k(&c);
        ret(sock_fd(k, fd).and_then(|(_held, raw)| {
            check(unsafe { libc::listen(raw, backlog) })?;
            crate::idle::listening(raw);
            Ok(0)
        }))
    })?;

    linker.func_wrap(m, "accept", |c: Caller<'_, Ctx>, fd: i32, addr: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let (_held, raw) = sock_fd(k, fd)?;
            let mut st: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut l = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            let nfd = check(unsafe { libc::accept(raw, &mut st as *mut _ as *mut libc::sockaddr, &mut l) })?;
            let gfd = adopt_socket(k, nfd, false)?;
            crate::idle::accepted(raw, nfd);
            if let Ok(sa) = from_host(&st) {
                if let Err(e) = write_addr(k, sa, addr as u32, len as u32) {
                    // The guest never learns about gfd, so it must not stay in the table.
                    drop(k.remove(gfd));
                    return Err(e);
                }
            }
            Ok(gfd)
        })())
    })?;

    linker.func_wrap(m, "connect", |c: Caller<'_, Ctx>, fd: i32, addr: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let (_held, raw) = sock_fd(k, fd)?;
            let sa = k.policy.connect(read_addr(k, addr as u32, len as u32)?)?;
            let (st, l) = to_host(sa);
            check(unsafe { libc::connect(raw, &st as *const _ as *const libc::sockaddr, l) })
        })())
    })?;

    linker.func_wrap(m, "getsockopt", |c: Caller<'_, Ctx>, fd: i32, level: i32, name: i32, val: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let (_held, raw) = sock_fd(k, fd)?;
            let room = k.mem.u32(len as u32)?;
            let Some((hl, hn)) = sockopt(level, name) else {
                k.mem.write(val as u32, &vec![0u8; room.min(16) as usize])?;
                return Ok(0);
            };
            let mut buf = [0u8; 16];
            let mut l = room.min(16) as libc::socklen_t;
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
            let (_held, raw) = sock_fd(k, fd)?;
            let Some((hl, hn)) = sockopt(level, name) else { return Ok(0) };
            let v = k.mem.read(val as u32, (len as u32).min(16))?;
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
            let (_held, raw) = sock_fd(k, fd)?;
            let p = k.mem.ptr(buf as u32, len as u32)?;
            let mut st: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut l = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            let r =
                unsafe { libc::recvfrom(raw, p as *mut libc::c_void, len as u32 as usize, msg_flags(flags), &mut st as *mut _ as *mut libc::sockaddr, &mut l) };
            if r < 0 {
                return Err(errno::last());
            }
            if addr != 0 && l > 0 {
                if let Ok(sa) = from_host(&st) {
                    write_addr(k, sa, addr as u32, alen as u32)?;
                }
            }
            Ok(r as i32)
        })())
    })?;

    linker.func_wrap(m, "sendto", |c: Caller<'_, Ctx>, fd: i32, buf: i32, len: i32, flags: i32, addr: i32, alen: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let (_held, raw) = sock_fd(k, fd)?;
            let p = k.mem.ptr(buf as u32, len as u32)?;
            let r = if addr == 0 {
                unsafe { libc::send(raw, p as *const libc::c_void, len as u32 as usize, msg_flags(flags)) }
            } else {
                let sa = k.policy.connect(read_addr(k, addr as u32, alen as u32)?)?;
                let (st, l) = to_host(sa);
                unsafe { libc::sendto(raw, p as *const libc::c_void, len as u32 as usize, msg_flags(flags), &st as *const _ as *const libc::sockaddr, l) }
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
            let (_held, raw) = sock_fd(k, fd)?;
            check(unsafe { libc::shutdown(raw, shutdown_how(how)?) })
        })())
    })?;

    linker.func_wrap(m, "pipe", |c: Caller<'_, Ctx>, fds: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            k.mem.ptr(fds as u32, 8)?;
            let mut p = [0 as libc::c_int; 2];
            check(unsafe { libc::pipe(p.as_mut_ptr()) })?;
            // Owned at once, so both ends close on any error below.
            let (rd, wr) = unsafe { (OwnedFd::from_raw_fd(p[0]), OwnedFd::from_raw_fd(p[1])) };
            for fd in p {
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
            }
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

    linker.func_wrap(m, "gethostname", |c: Caller<'_, Ctx>, name: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let mut buf = [0u8; 256];
            check(unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) })?;
            let n = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            if n + 1 > len as usize {
                return Err(errno::NAMETOOLONG);
            }
            k.mem.write(name as u32, &buf[..n + 1])?;
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
    let (_held, raw) = sock_fd(k, fd)?;
    let mut st: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut l = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    let p = &mut st as *mut _ as *mut libc::sockaddr;
    check(unsafe {
        if peer {
            libc::getpeername(raw, p, &mut l)
        } else {
            libc::getsockname(raw, p, &mut l)
        }
    })?;
    let sa = from_host(&st)?;
    let sa = if peer { sa } else { k.policy.guest_local(sa) };
    write_addr(k, sa, addr, len)?;
    Ok(0)
}
