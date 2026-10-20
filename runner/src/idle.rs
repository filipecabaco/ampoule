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
//!
//! `--restart on-failure` (`restart` in ampoule.toml) uses the same supervisor, with or without the
//! idle stop: a child that exits non-zero, out of memory included, is started again after a backoff,
//! on the listeners the supervisor holds, until it fails `MAX_RESTARTS` times within
//! `RESTART_WINDOW`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

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
    ctl: OwnedFd,
    /// None: supervised only to be restarted (`--restart`), never stopped for idleness.
    after: Option<Duration>,
    /// Listeners the supervisor handed down, by guest port.
    inherited: HashMap<u16, OwnedFd>,
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
pub fn init_child(after: Option<Duration>) -> Result<()> {
    let ctl: RawFd = std::env::var(CHILD_ENV).context("not an idle-stop child")?.parse().context(CHILD_ENV)?;
    let mut inherited = HashMap::new();
    for pair in std::env::var(LISTEN_ENV).unwrap_or_default().split(',').filter(|p| !p.is_empty()) {
        let (port, fd) = pair.split_once('=').with_context(|| format!("bad {LISTEN_ENV} entry {pair:?}"))?;
        let fd: RawFd = fd.parse().with_context(|| format!("bad {LISTEN_ENV} entry {pair:?}"))?;
        // SAFETY: the supervisor handed this listener down for this process alone to own.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        cloexec(&fd, true);
        inherited.insert(port.parse().with_context(|| format!("bad {LISTEN_ENV} entry {pair:?}"))?, fd);
    }
    // SAFETY: as above, for the control socket.
    let ctl = unsafe { OwnedFd::from_raw_fd(ctl) };
    cloexec(&ctl, true);
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
    let held = c.inherited.get(&port)?.as_raw_fd();
    if let Err(e) = crate::fds::dup_onto(held, raw) {
        return Some(Err(e));
    }
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
pub fn listening(fd: BorrowedFd<'_>) {
    let raw = fd.as_raw_fd();
    let Some(c) = CHILD.get() else { return };
    let Some(port) = c.bound.lock().unwrap().get(&raw).copied() else { return };
    c.listeners.lock().unwrap().insert(raw);
    if c.sent.lock().unwrap().insert(port) {
        if let Err(e) = send(&c.ctl, LISTENER, port, Some(fd)) {
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
    let Some((c, after)) = CHILD.get().and_then(|c| Some((c, c.after?))) else { return Ok(()) };
    std::thread::Builder::new().name("beam-idle".into()).spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1).min(after));
            if !c.armed.load(Ordering::Relaxed) || !c.inbound.lock().unwrap().is_empty() {
                continue;
            }
            if c.last.lock().unwrap().elapsed() >= after {
                break;
            }
        }
        eprintln!("ampoule: no connections for {}, stopping the VM until the next one", human(after));
        // The supervisor reads this once the child has exited, so it waits instead of exiting too.
        let _ = send(&c.ctl, IDLE, 0, None);
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

/// Restarts allowed within `RESTART_WINDOW` before the supervisor gives up and exits with the VM's
/// status, leaving the next restart to systemd, Kubernetes or Docker.
const MAX_RESTARTS: usize = 5;
const RESTART_WINDOW: Duration = Duration::from_secs(300);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Run the VM in child processes: with `after`, stopping it when idle and starting it again on a
/// connection; with `restart`, starting it again when it fails (exits non-zero, a crash or running
/// out of memory included). The listeners stay open in between, so connections wait in the backlog
/// instead of being refused. Returns the exit status of the last child.
pub fn supervise(after: Option<Duration>, restart: bool) -> Result<i32> {
    let (mine, theirs) = control_pair().context("socketpair")?;
    cloexec(&mine, true);
    cloexec(&theirs, false);

    let child_pid = Arc::new(AtomicI32::new(0));
    let stopping = Arc::new(AtomicBool::new(false));
    forward_signals(child_pid.clone(), stopping.clone())?;

    let exe = std::env::current_exe()?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let mut held: BTreeMap<u16, OwnedFd> = BTreeMap::new();
    if let Some(after) = after {
        eprintln!("ampoule: stopping the VM after {} without connections", human(after));
    }
    let mut restarts: Vec<Instant> = Vec::new();
    loop {
        let listen = held.iter().map(|(p, fd)| format!("{p}={}", fd.as_raw_fd())).collect::<Vec<_>>().join(",");
        for fd in held.values() {
            cloexec(fd, false);
        }
        let spawned = std::process::Command::new(&exe).args(&args).env(CHILD_ENV, theirs.as_raw_fd().to_string()).env(LISTEN_ENV, listen).spawn();
        for fd in held.values() {
            cloexec(fd, true);
        }
        let mut child = spawned.context("starting the VM")?;
        child_pid.store(child.id() as i32, Ordering::SeqCst);
        let status = child.wait()?;
        child_pid.store(0, Ordering::SeqCst);

        // Everything the child sent, listeners included, is in the socket buffer until read.
        let idle = drain(&mine, &mut held);
        let code = status.code().unwrap_or_else(|| 128 + std::os::unix::process::ExitStatusExt::signal(&status).unwrap_or(0));
        if stopping.load(Ordering::SeqCst) {
            return Ok(code);
        }
        if !idle || held.is_empty() {
            if !restart || code == 0 {
                return Ok(code);
            }
            let now = Instant::now();
            restarts.retain(|t| now.duration_since(*t) < RESTART_WINDOW);
            if restarts.len() >= MAX_RESTARTS {
                eprintln!("ampoule: VM failed (status {code}) {} times in {}; giving up", restarts.len() + 1, human(RESTART_WINDOW));
                return Ok(code);
            }
            let backoff = Duration::from_secs(1 << restarts.len()).min(MAX_BACKOFF);
            restarts.push(now);
            eprintln!(
                "ampoule: VM failed (status {code}); restarting in {} ({} of {MAX_RESTARTS} in {})",
                human(backoff),
                restarts.len(),
                human(RESTART_WINDOW)
            );
            std::thread::sleep(backoff);
            if stopping.load(Ordering::SeqCst) {
                return Ok(code);
            }
            continue;
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
fn drain(ctl: impl AsFd, held: &mut BTreeMap<u16, OwnedFd>) -> bool {
    let ctl = ctl.as_fd();
    let mut idle = false;
    loop {
        match recv(ctl) {
            Some((LISTENER, port, Some(fd))) => {
                cloexec(&fd, true);
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
    use rustix::event::{PollFd, PollFlags};
    let ports: Vec<u16> = held.keys().copied().collect();
    let mut fds: Vec<PollFd<'_>> = held.values().map(|fd| PollFd::new(fd, PollFlags::IN)).collect();
    loop {
        match rustix::event::poll(&mut fds, None) {
            Ok(_) => {},
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(std::io::Error::from(e)).context("waiting for a connection"),
        }
        if let Some(i) = fds.iter().position(|p| !p.revents().is_empty()) {
            return Ok(ports[i]);
        }
    }
}

/// SIGTERM, SIGINT and SIGQUIT go to the running child; with none running, the supervisor exits.
fn forward_signals(child_pid: Arc<AtomicI32>, stopping: Arc<AtomicBool>) -> Result<()> {
    // SAFETY: sigset_t is plain data, initialised by sigemptyset before use. Raw libc: rustix offers
    // signal masks only in its unstable runtime module.
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
        // SAFETY: `set` was built above; sigwait writes one int.
        if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
            continue;
        }
        stopping.store(true, Ordering::SeqCst);
        match child_pid.load(Ordering::SeqCst) {
            0 => std::process::exit(0),
            pid => {
                if let (Some(pid), Some(sig)) = (rustix::process::Pid::from_raw(pid), rustix::process::Signal::from_named_raw(sig)) {
                    let _ = rustix::process::kill_process(pid, sig);
                }
            },
        }
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The control socket.

fn control_pair() -> std::io::Result<(OwnedFd, OwnedFd)> {
    use rustix::net::{AddressFamily, SocketFlags, SocketType};
    Ok(rustix::net::socketpair(AddressFamily::UNIX, SocketType::STREAM, SocketFlags::empty(), None)?)
}

fn send(sock: impl AsFd, tag: u8, port: u16, fd: Option<BorrowedFd<'_>>) -> std::io::Result<()> {
    use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags};
    let p = port.to_le_bytes();
    let data = [tag, p[0], p[1]];
    let mut space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    let fds = fd.map(|f| [f]);
    if let Some(fds) = &fds {
        control.push(SendAncillaryMessage::ScmRights(fds));
    }
    rustix::net::sendmsg(sock, &[std::io::IoSlice::new(&data)], &mut control, SendFlags::empty())?;
    Ok(())
}

/// One message without waiting, or None when there is none.
fn recv(sock: impl AsFd) -> Option<(u8, u16, Option<OwnedFd>)> {
    use rustix::net::{RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags};
    let mut data = [0u8; MSG];
    let mut space = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    let got = rustix::net::recvmsg(sock, &mut [std::io::IoSliceMut::new(&mut data)], &mut control, RecvFlags::DONTWAIT).ok()?;
    let mut fd = None;
    for msg in control.drain() {
        if let RecvAncillaryMessage::ScmRights(mut fds) = msg {
            fd = fd.or_else(|| fds.next());
        }
    }
    if got.bytes != MSG {
        return None;
    }
    Some((data[0], u16::from_le_bytes([data[1], data[2]]), fd))
}

fn cloexec(fd: impl AsFd, on: bool) {
    use rustix::io::FdFlags;
    if let Ok(mut flags) = rustix::io::fcntl_getfd(&fd) {
        flags.set(FdFlags::CLOEXEC, on);
        let _ = rustix::io::fcntl_setfd(&fd, flags);
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
        let (a, b) = control_pair().unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        send(&a, LISTENER, 4000, Some(listener.as_fd())).unwrap();
        send(&a, IDLE, 0, None).unwrap();
        drop(listener);
        let mut held = BTreeMap::new();
        assert!(drain(&b, &mut held));
        // The received descriptor keeps the socket listening after the sender closed its own.
        let fd = held.get(&4000).expect("listener received").as_raw_fd();
        let _client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        assert_eq!(wait_readable(&held).unwrap(), 4000);
        assert!(fd >= 0);
        assert!(!drain(&b, &mut BTreeMap::new()));
    }
}
