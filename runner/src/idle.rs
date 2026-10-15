//! Stopping the VM when nothing is connected and starting it again on the next connection
//! (`--idle-stop`, `AMPOULE_IDLE_STOP`, `idle_stop` in ampoule.toml), so an idle app costs a few MB.
//!
//! The process that runs `start` becomes a supervisor: it runs the VM in a child process (this same
//! binary with the same arguments) and talks to it over a Unix socket pair.
//!
//! - When the guest listens on a fixed port, the child passes that listening socket to the
//!   supervisor (SCM_RIGHTS). The supervisor's reference keeps it open after the guest closes it.
//! - The child counts the connections accepted on those listeners that are still open. After none
//!   for the idle time, it tells the supervisor and stops the guest the way SIGTERM does
//!   (`init:stop()`).
//! - The supervisor then waits on the listeners. A new connection stays queued in the listen backlog
//!   while it starts another child, whose guest takes over the socket it binds to that port instead
//!   of binding a new one, and accepts the connection once the app listens.
//!
//! A connection that arrives between the decision to stop and the guest closing its listener can be
//! accepted by an app that is already shutting down, and dropped.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use crate::errno;
use crate::kernel::Kernel;
use crate::mem::R;

/// The supervisor's end of the control socket, as seen by the child.
const CHILD_ENV: &str = "AMPOULE_IDLE_CHILD";
/// Listening sockets handed down to the child: `port=fd,...`, by guest port.
const LISTEN_ENV: &str = "AMPOULE_IDLE_LISTEN";

/// A message on the control socket: a tag and a guest port, with a file descriptor for `LISTENER`.
const LISTENER: u8 = b'L';
const IDLE: u8 = b'I';
const MSG: usize = 3;

/// "300", "90s", "5m", "1h"; "0" and "off" turn it off.
pub fn parse_duration(s: &str) -> Result<Option<Duration>> {
    let s = s.trim();
    if s.is_empty() || s == "0" || s.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = num.parse().with_context(|| format!("bad duration {s:?} (examples: 300, 90s, 5m, 1h)"))?;
    let secs = match unit {
        "" | "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => bail!("bad duration {s:?} (examples: 300, 90s, 5m, 1h)"),
    };
    Ok((secs > 0).then(|| Duration::from_secs(secs)))
}

pub fn is_child() -> bool {
    std::env::var_os(CHILD_ENV).is_some()
}

// ---------------------------------------------------------------------------------------------
// The child: hooks from the socket calls, and the watch that stops an idle guest.

struct Child {
    ctl: RawFd,
    after: Duration,
    /// Listeners the supervisor handed down, by guest port.
    inherited: HashMap<u16, RawFd>,
    /// Ports whose listener the supervisor already holds.
    sent: Mutex<HashSet<u16>>,
    /// Guest sockets bound to a fixed port, by host fd.
    bound: Mutex<HashMap<RawFd, u16>>,
    /// Those of them listening.
    listeners: Mutex<HashSet<RawFd>>,
    /// Connections accepted on a listener and still open.
    inbound: Mutex<HashSet<RawFd>>,
    last: Mutex<Instant>,
    armed: AtomicBool,
}

static CHILD: OnceLock<Child> = OnceLock::new();

/// In a child the supervisor started: take what it handed down.
pub fn init_child(after: Duration) -> Result<()> {
    let ctl: RawFd = std::env::var(CHILD_ENV).context("not an idle-stop child")?.parse().context(CHILD_ENV)?;
    let mut inherited = HashMap::new();
    for pair in std::env::var(LISTEN_ENV).unwrap_or_default().split(',').filter(|p| !p.is_empty()) {
        let (port, fd) = pair.split_once('=').with_context(|| format!("bad {LISTEN_ENV} entry {pair:?}"))?;
        let fd: RawFd = fd.parse().with_context(|| format!("bad {LISTEN_ENV} entry {pair:?}"))?;
        cloexec(fd, true);
        inherited.insert(port.parse().with_context(|| format!("bad {LISTEN_ENV} entry {pair:?}"))?, fd);
    }
    cloexec(ctl, true);
    let sent = Mutex::new(inherited.keys().copied().collect());
    let child = Child {
        ctl,
        after,
        inherited,
        sent,
        bound: Mutex::default(),
        listeners: Mutex::default(),
        inbound: Mutex::default(),
        last: Mutex::new(Instant::now()),
        armed: AtomicBool::new(false),
    };
    if CHILD.set(child).is_err() {
        bail!("idle stop started twice");
    }
    Ok(())
}

/// `bind` to `port`: if the supervisor handed down its listener, put it in place of the guest's new
/// socket instead of binding (the guest's socket options were set on the same kind of socket).
pub fn take_over(port: u16, raw: RawFd) -> Option<R<()>> {
    let c = CHILD.get()?;
    let held = *c.inherited.get(&port)?;
    if unsafe { libc::dup2(held, raw) } < 0 {
        return Some(Err(errno::last()));
    }
    cloexec(raw, true);
    c.bound.lock().unwrap().insert(raw, port);
    Some(Ok(()))
}

/// A guest socket bound to a fixed port.
pub fn bound(raw: RawFd, port: u16) {
    if let (Some(c), true) = (CHILD.get(), port != 0) {
        c.bound.lock().unwrap().insert(raw, port);
    }
}

/// A guest socket started listening: hand it to the supervisor the first time, and start counting.
pub fn listening(raw: RawFd) {
    let Some(c) = CHILD.get() else { return };
    let Some(port) = c.bound.lock().unwrap().get(&raw).copied() else { return };
    c.listeners.lock().unwrap().insert(raw);
    if c.sent.lock().unwrap().insert(port) {
        if let Err(e) = send(c.ctl, LISTENER, port, Some(raw)) {
            eprintln!("ampoule: cannot hand the listener on port {port} to the supervisor ({e}); the VM will not stop when idle");
            return;
        }
    }
    *c.last.lock().unwrap() = Instant::now();
    c.armed.store(true, Ordering::Relaxed);
}

/// A connection accepted on `listener`.
pub fn accepted(listener: RawFd, conn: RawFd) {
    let Some(c) = CHILD.get() else { return };
    if c.listeners.lock().unwrap().contains(&listener) {
        c.inbound.lock().unwrap().insert(conn);
        *c.last.lock().unwrap() = Instant::now();
    }
}

/// A listener the supervisor also holds: shutting it down would stop it accepting for the next VM
/// too, since shutdown(2) acts on the socket, not on one descriptor.
pub fn held(raw: RawFd) -> bool {
    CHILD.get().is_some_and(|c| c.listeners.lock().unwrap().contains(&raw))
}

/// A guest socket closed (its entry dropped, before the host fd closes).
pub fn closed(raw: RawFd) {
    let Some(c) = CHILD.get() else { return };
    if c.inbound.lock().unwrap().remove(&raw) {
        *c.last.lock().unwrap() = Instant::now();
    }
    c.listeners.lock().unwrap().remove(&raw);
    c.bound.lock().unwrap().remove(&raw);
}

/// Watch for idleness and stop the guest once it is; `stop_timeout` bounds the stop.
pub fn watch(k: Arc<Kernel>, stop_timeout: Duration) -> std::io::Result<()> {
    let Some(c) = CHILD.get() else { return Ok(()) };
    std::thread::Builder::new().name("beam-idle".into()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1).min(c.after));
            if !c.armed.load(Ordering::Relaxed) || !c.inbound.lock().unwrap().is_empty() {
                continue;
            }
            if c.last.lock().unwrap().elapsed() >= c.after {
                break;
            }
        }
        eprintln!("ampoule: no connections for {}, stopping the VM until the next one", human(c.after));
        // The supervisor reads this once the child has exited, so it waits instead of exiting too.
        let _ = send(c.ctl, IDLE, 0, None);
        if crate::signals::deliver(&k, crate::signals::GUEST_SIGTERM) {
            std::thread::sleep(stop_timeout);
            eprintln!("ampoule: still running {} after the idle stop, exiting", human(stop_timeout));
        }
        std::process::exit(0);
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The supervisor.

/// Run the VM in child processes, stopping when idle and starting again on a connection. Returns
/// the exit status of the last child that stopped for another reason than idleness.
pub fn supervise(after: Duration) -> Result<i32> {
    let mut pair = [0 as libc::c_int; 2];
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error()).context("socketpair");
    }
    let (mine, theirs) = unsafe { (OwnedFd::from_raw_fd(pair[0]), OwnedFd::from_raw_fd(pair[1])) };
    cloexec(mine.as_raw_fd(), true);
    cloexec(theirs.as_raw_fd(), false);

    let child_pid = Arc::new(AtomicI32::new(0));
    let stopping = Arc::new(AtomicBool::new(false));
    forward_signals(child_pid.clone(), stopping.clone())?;

    let exe = std::env::current_exe()?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let mut held: BTreeMap<u16, OwnedFd> = BTreeMap::new();
    eprintln!("ampoule: stopping the VM after {} without connections", human(after));
    loop {
        let listen = held.iter().map(|(p, fd)| format!("{p}={}", fd.as_raw_fd())).collect::<Vec<_>>().join(",");
        for fd in held.values() {
            cloexec(fd.as_raw_fd(), false);
        }
        let spawned = std::process::Command::new(&exe).args(&args).env(CHILD_ENV, theirs.as_raw_fd().to_string()).env(LISTEN_ENV, listen).spawn();
        for fd in held.values() {
            cloexec(fd.as_raw_fd(), true);
        }
        let mut child = spawned.context("starting the VM")?;
        child_pid.store(child.id() as i32, Ordering::SeqCst);
        let status = child.wait()?;
        child_pid.store(0, Ordering::SeqCst);

        // Everything the child sent, listeners included, is in the socket buffer until read.
        let idle = drain(mine.as_raw_fd(), &mut held);
        let code = status.code().unwrap_or_else(|| 128 + std::os::unix::process::ExitStatusExt::signal(&status).unwrap_or(0));
        if stopping.load(Ordering::SeqCst) || !idle || held.is_empty() {
            return Ok(code);
        }

        let ports: Vec<String> = held.keys().map(u16::to_string).collect();
        eprintln!("ampoule: VM stopped; waiting for a connection on port {}", ports.join(", "));
        let port = wait_readable(&held)?;
        if stopping.load(Ordering::SeqCst) {
            return Ok(0);
        }
        eprintln!("ampoule: connection on port {port}, starting the VM");
    }
}

/// Read every pending message: listeners into `held`; true if one said the child stopped for idleness.
fn drain(ctl: RawFd, held: &mut BTreeMap<u16, OwnedFd>) -> bool {
    let mut idle = false;
    loop {
        match recv(ctl) {
            Some((LISTENER, port, Some(fd))) => {
                cloexec(fd.as_raw_fd(), true);
                held.insert(port, fd);
            },
            Some((IDLE, _, _)) => idle = true,
            Some(_) => {},
            None => return idle,
        }
    }
}

/// Block until one of the held listeners has a connection waiting; returns its port.
fn wait_readable(held: &BTreeMap<u16, OwnedFd>) -> Result<u16> {
    let ports: Vec<u16> = held.keys().copied().collect();
    let mut fds: Vec<libc::pollfd> = held.values().map(|fd| libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }).collect();
    loop {
        let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e).context("waiting for a connection");
        }
        if let Some(i) = fds.iter().position(|p| p.revents != 0) {
            return Ok(ports[i]);
        }
    }
}

/// SIGTERM, SIGINT and SIGQUIT go to the running child; with none running, the supervisor exits.
fn forward_signals(child_pid: Arc<AtomicI32>, stopping: Arc<AtomicBool>) -> Result<()> {
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut set);
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGQUIT] {
            libc::sigaddset(&mut set, s);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
    std::thread::Builder::new().name("idle-signals".into()).spawn(move || loop {
        let mut sig: libc::c_int = 0;
        if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
            continue;
        }
        stopping.store(true, Ordering::SeqCst);
        match child_pid.load(Ordering::SeqCst) {
            0 => std::process::exit(0),
            pid => unsafe {
                libc::kill(pid, sig);
            },
        }
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The control socket.

fn send(sock: RawFd, tag: u8, port: u16, fd: Option<RawFd>) -> std::io::Result<()> {
    let p = port.to_le_bytes();
    let mut data = [tag, p[0], p[1]];
    let mut iov = libc::iovec { iov_base: data.as_mut_ptr().cast(), iov_len: MSG };
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut cbuf = vec![0u8; space];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    if let Some(fd) = fd {
        msg.msg_control = cbuf.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        unsafe {
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
            std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>(), fd);
        }
    }
    if unsafe { libc::sendmsg(sock, &msg, 0) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// One message without waiting, or None when there is none.
fn recv(sock: RawFd) -> Option<(u8, u16, Option<OwnedFd>)> {
    let mut data = [0u8; MSG];
    let mut iov = libc::iovec { iov_base: data.as_mut_ptr().cast(), iov_len: MSG };
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut cbuf = vec![0u8; space];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = cbuf.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    let n = unsafe { libc::recvmsg(sock, &mut msg, libc::MSG_DONTWAIT) };
    if n != MSG as isize {
        return None;
    }
    let mut fd = None;
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if !cmsg.is_null() && (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
            fd = Some(OwnedFd::from_raw_fd(std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>())));
        }
    }
    Some((data[0], u16::from_le_bytes([data[1], data[2]]), fd))
}

fn cloexec(fd: RawFd, on: bool) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        let flags = if on { flags | libc::FD_CLOEXEC } else { flags & !libc::FD_CLOEXEC };
        libc::fcntl(fd, libc::F_SETFD, flags);
    }
}

fn human(d: Duration) -> String {
    match d.as_secs() {
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_take_seconds_minutes_and_hours() {
        let d = |s| parse_duration(s).unwrap();
        assert_eq!(
            (d("300"), d("90s"), d("5m"), d("1h")),
            (Some(Duration::from_secs(300)), Some(Duration::from_secs(90)), Some(Duration::from_secs(300)), Some(Duration::from_secs(3600)))
        );
        assert_eq!((d("0"), d("off"), d("")), (None, None, None));
        assert!(parse_duration("5 minutes").is_err());
        assert!(parse_duration("m").is_err());
    }

    #[test]
    fn listeners_and_idleness_cross_the_control_socket() {
        let mut pair = [0 as libc::c_int; 2];
        assert_eq!(unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, pair.as_mut_ptr()) }, 0);
        let (a, b) = unsafe { (OwnedFd::from_raw_fd(pair[0]), OwnedFd::from_raw_fd(pair[1])) };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        send(a.as_raw_fd(), LISTENER, 4000, Some(listener.as_raw_fd())).unwrap();
        send(a.as_raw_fd(), IDLE, 0, None).unwrap();
        drop(listener);
        let mut held = BTreeMap::new();
        assert!(drain(b.as_raw_fd(), &mut held));
        // The received descriptor keeps the socket listening after the sender closed its own.
        let fd = held.get(&4000).expect("listener received").as_raw_fd();
        let _client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        assert_eq!(wait_readable(&held).unwrap(), 4000);
        assert!(fd >= 0);
        assert!(!drain(b.as_raw_fd(), &mut BTreeMap::new()));
    }
}
