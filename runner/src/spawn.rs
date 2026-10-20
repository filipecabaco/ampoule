//! Host processes for the guest: `open_port({spawn, ...})`, `open_port({spawn_executable, ...})`
//! and everything built on them (`System.cmd/3`, `os:cmd/1`).
//!
//! ERTS starts every program through erl_child_setup, a helper it forks at boot: for each port it
//! sends the helper a Start request with the port's pipes, the helper forks, sends Go with the
//! child's pid, the child reads the command from its stdin pipe, waits for the VM's Ack and execs;
//! when the child exits the helper reports SigChld with its wait status. A wasm guest cannot fork,
//! so the runtime plays the helper (see engine/patches/erts-wasi.patch): `forker` gives ERTS the
//! pipe SigChld messages arrive on and `forker_write` takes each Start request. A host thread per
//! request reads the command, checks it against the policy, starts the program with the port's
//! pipes as its stdio, and waits for it.
//!
//! The start is a plain fork and execve, as erl_child_setup's: the child must wait for the VM's
//! Ack between the two, which `std::process::Command` cannot do without holding up the fork lock
//! (see `fds`) until the guest answers. Everything the child touches is allocated before the
//! fork, and it makes only async-signal-safe calls until execve.
//!
//! The program runs on the host, not in the guest's tree: the executable and the working
//! directories are translated from guest paths, but arguments and environment pass as they are
//! (except `HOME`, the guest's /tmp). Only what the policy allows runs (`[process] allow`,
//! `--allow-exec`), and each allowed executable or directory is also mounted read-only at its host
//! path, so `os:find_executable` (which `System.cmd/3` calls) finds it in the guest.

use std::ffi::{c_char, CString};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use wasmtime::{Caller, Linker};

use crate::errno;
use crate::kernel::{normalize, Entry, Kernel, Kind, Mount};
use crate::mem::R;
use crate::net::ret;
use crate::{k, Ctx};

/// `sizeof(ErtsSysForkerProto)` in the guest: an enum, then a union of at most a 32-bit port id
/// and three ints.
const PROTO_LEN: usize = 20;
const ACTION_START: u32 = 0;
const ACTION_GO: u32 = 2;
const ACTION_SIGCHLD: u32 = 3;
/// THE_NON_VALUE: the port asked for no exit status.
const NO_PORT: u32 = 0;

/// The largest command block (program, arguments, environment) accepted: above any host's ARG_MAX.
const MAX_REQUEST: usize = 4 << 20;
/// Programs running at once (each has a host thread waiting for it); more fail with EAGAIN.
const MAX_CHILDREN: usize = 1024;
static CHILDREN: AtomicUsize = AtomicUsize::new(0);

/// One of the MAX_CHILDREN slots, freed when its thread ends.
struct Live;

impl Drop for Live {
    fn drop(&mut self) {
        CHILDREN.fetch_sub(1, Ordering::Relaxed);
    }
}

const FLAG_USE_STDIO: u32 = 1;
const FLAG_DO_READ: u32 = 4;
const FLAG_DO_WRITE: u32 = 8;

/// What the guest may run: host executables, and directories of them, by canonical host path.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Exec {
    pub allowed: Vec<Allowed>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Allowed {
    /// Where the guest sees it: the path as written, or as found on PATH.
    pub guest: String,
    pub host: PathBuf,
    pub dir: bool,
}

impl Exec {
    /// Resolve `allow` entries on this host: `*` is every directory on `path`, a bare name is found
    /// on `path` as a shell would, an absolute path is taken as is. Entries this host lacks are
    /// left out, so the app finds no such program, as it would natively.
    pub fn resolve(allow: &[String], path: &str) -> Exec {
        let dirs: Vec<&str> = path.split(':').filter(|d| d.starts_with('/')).collect();
        let mut allowed: Vec<Allowed> = Vec::new();
        let mut add = |guest: &str, host: &Path, dir: bool| {
            let guest = normalize(guest);
            if !allowed.iter().any(|a| a.guest == guest) {
                allowed.push(Allowed { guest, host: host.to_path_buf(), dir });
            }
        };
        for entry in allow {
            if entry == "*" {
                for d in &dirs {
                    if let Some(host) = std::fs::canonicalize(d).ok().filter(|h| h.is_dir()) {
                        add(d, &host, true);
                    }
                }
            } else if entry.starts_with('/') {
                if let Some(host) = executable(Path::new(entry)) {
                    add(entry, &host, false);
                }
            } else if let Some((guest, host)) = dirs.iter().map(|d| format!("{d}/{entry}")).find_map(|p| executable(Path::new(&p)).map(|h| (p, h))) {
                add(&guest, &host, false);
            }
        }
        Exec { allowed }
    }

    /// The path to execute for `host`, if the guest may run it: the canonical file, when that is an
    /// allowed one; or `host` in its canonical directory, when that is an allowed one (wherever a
    /// symlink there points, as with Homebrew's bin/). The caller checks and executes this same
    /// path, so nothing the guest changes in between can redirect the exec.
    pub fn program(&self, host: &Path) -> Option<PathBuf> {
        let file = std::fs::canonicalize(host).ok()?;
        if self.allowed.iter().any(|a| !a.dir && a.host == file) {
            return Some(file);
        }
        let dir = std::fs::canonicalize(host.parent()?).ok()?;
        let in_dir = dir.join(host.file_name()?);
        self.allowed.iter().any(|a| a.dir && a.host == dir).then_some(in_dir)
    }

    /// Read-only mounts that show the allowed executables to the guest. Paths the runtime's own
    /// mounts own (the root, the release, /proc, /tmp) are left out.
    pub fn mounts(&self, reserved: &[&str]) -> Vec<Mount> {
        let owned = |g: &str| g == "/" || reserved.iter().any(|r| g == *r || g.starts_with(&format!("{r}/")));
        self.allowed
            .iter()
            .filter(|a| !owned(&a.guest))
            .map(|a| Mount { guest: a.guest.clone(), backend: crate::kernel::Backend::Host { path: a.host.clone(), ro: true } })
            .collect()
    }
}

/// `p` canonicalized, when it is a regular file with an execute bit.
fn executable(p: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let host = std::fs::canonicalize(p).ok()?;
    let md = std::fs::metadata(&host).ok()?;
    (md.is_file() && md.permissions().mode() & 0o111 != 0).then_some(host)
}

/// A Start request's command block, as erl_child_setup reads it from the child's stdin pipe.
#[derive(Debug, PartialEq)]
struct Request {
    flags: u32,
    /// The program (spawn_executable) or the shell command line (spawn).
    cmd: Vec<u8>,
    cwd: Vec<u8>,
    /// The port's `{cd, Dir}`.
    wd: Option<Vec<u8>>,
    env: Vec<Vec<u8>>,
    /// Present for spawn_executable only.
    argv: Option<Vec<Vec<u8>>>,
}

/// Parse the block after its length: big-endian flags, cmd, cwd, an optional wd, an empty string,
/// a big-endian count and that many environment strings, then (spawn_executable) a count and argv.
fn parse(b: &[u8]) -> Option<Request> {
    fn int(b: &[u8], at: &mut usize) -> Option<u32> {
        let v = u32::from_be_bytes(b.get(*at..*at + 4)?.try_into().ok()?);
        *at += 4;
        Some(v)
    }
    fn string(b: &[u8], at: &mut usize) -> Option<Vec<u8>> {
        let n = b.get(*at..)?.iter().position(|&c| c == 0)?;
        let s = b[*at..*at + n].to_vec();
        *at += n + 1;
        Some(s)
    }
    fn strings(b: &[u8], at: &mut usize) -> Option<Vec<Vec<u8>>> {
        let n = int(b, at)?;
        (0..n).map(|_| string(b, at)).collect()
    }
    let mut at = 0;
    let flags = int(b, &mut at)?;
    let cmd = string(b, &mut at)?;
    let cwd = string(b, &mut at)?;
    let wd = if *b.get(at)? == 0 { None } else { Some(string(b, &mut at)?) };
    at += 1;
    let env = strings(b, &mut at)?;
    let argv = if at < b.len() { Some(strings(b, &mut at)?) } else { None };
    (at == b.len()).then_some(Request { flags, cmd, cwd, wd, env, argv })
}

fn proto(action: u32, a: u32, b: i32) -> [u8; PROTO_LEN] {
    let mut p = [0u8; PROTO_LEN];
    p[0..4].copy_from_slice(&action.to_le_bytes());
    p[4..8].copy_from_slice(&a.to_le_bytes());
    p[8..12].copy_from_slice(&b.to_le_bytes());
    p
}

/// Raw write/read loops: also called between fork and exec, where only async-signal-safe calls
/// are allowed, on fd numbers (a BorrowedFd would need its own unsafe to make from them).
fn write_all(fd: RawFd, mut buf: &[u8]) -> bool {
    while !buf.is_empty() {
        // SAFETY: a host buffer and its length; the caller keeps `fd` open.
        let n = unsafe { libc::write(fd, buf.as_ptr().cast(), buf.len()) };
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if n <= 0 {
            return false;
        }
        buf = &buf[n as usize..];
    }
    true
}

fn read_exact(fd: RawFd, buf: &mut [u8]) -> bool {
    let mut at = 0;
    while at < buf.len() {
        // SAFETY: as in write_all.
        let n = unsafe { libc::read(fd, buf[at..].as_mut_ptr().cast(), buf.len() - at) };
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if n <= 0 {
            return false;
        }
        at += n as usize;
    }
    true
}

fn read_request(fd: RawFd) -> Option<Request> {
    let mut len = [0u8; 4];
    // The length goes in the guest's byte order (little-endian, as wasm is), the rest big-endian.
    if !read_exact(fd, &mut len) {
        return None;
    }
    let len = usize::try_from(i32::from_le_bytes(len)).ok().filter(|&n| n <= MAX_REQUEST)?;
    let mut block = vec![0u8; len];
    read_exact(fd, &mut block).then(|| parse(&block)).flatten()
}

/// The guest path `p`, relative to the guest directory `cwd`.
fn guest_abs(cwd: &[u8], p: &[u8]) -> String {
    let (cwd, p) = (String::from_utf8_lossy(cwd), String::from_utf8_lossy(p));
    if p.starts_with('/') {
        normalize(&p)
    } else {
        normalize(&format!("{cwd}/{p}"))
    }
}

/// A host directory for a guest one, or the errno refusing it.
fn host_dir(k: &Kernel, guest: &str) -> R<PathBuf> {
    let host = k.host_path(guest)?;
    if host.is_dir() {
        Ok(host)
    } else {
        Err(errno::NOTDIR)
    }
}

/// A program ready to start. Everything the child needs is here, allocated before the fork.
struct Prepared {
    program: CString,
    argv: Vec<CString>,
    env: Vec<CString>,
    /// The directory to run in, and whether failing to enter it fails the start (`{cd, Dir}`).
    dir: Option<(CString, bool)>,
}

fn cstring(b: &[u8]) -> R<CString> {
    CString::new(b).map_err(|_| errno::INVAL)
}

/// What to run for `req`, or the errno open_port reports.
fn prepare(k: &Kernel, req: &Request) -> R<Prepared> {
    let (program, argv) = match &req.argv {
        Some(argv) => (k.host_path(&guest_abs(&req.cwd, &req.cmd))?, argv.iter().map(|a| cstring(a)).collect::<R<Vec<_>>>()?),
        None => (PathBuf::from("/bin/sh"), vec![cstring(b"sh")?, cstring(b"-c")?, cstring(&req.cmd)?]),
    };
    // Not inside a mount the guest writes to, where it could swap the file or a symlink to it.
    let Some(program) = k.policy.exec.program(&program).filter(|p| !k.guest_writable(p)) else {
        if crate::trace() {
            eprintln!("[trace] spawn refused: {} is not allowed", program.display());
        }
        return Err(errno::ACCES);
    };
    let mut env = Vec::with_capacity(req.env.len());
    for var in &req.env {
        // The guest's HOME is its /tmp: give the program the host directory behind it.
        let host_home = var.strip_prefix(b"HOME=").and_then(|v| k.host_path(&String::from_utf8_lossy(v)).ok());
        env.push(match host_home {
            Some(home) => cstring(&[b"HOME=", home.as_os_str().as_bytes()].concat())?,
            None => cstring(var)?,
        });
    }
    // A cwd the host lacks is ignored, as erl_child_setup ignores a failed chdir to it; a `{cd,
    // Dir}` the host lacks fails the open_port.
    let dir = match &req.wd {
        Some(wd) => Some((host_dir(k, &guest_abs(&req.cwd, wd))?, true)),
        None => host_dir(k, &String::from_utf8_lossy(&req.cwd)).ok().map(|d| (d, false)),
    };
    let dir = match dir {
        Some((d, required)) => Some((cstring(d.as_os_str().as_bytes())?, required)),
        None => None,
    };
    Ok(Prepared { program: cstring(program.as_os_str().as_bytes())?, argv, env, dir })
}

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO)
}

/// The child, between fork and execve: erl_child_setup's child, with only async-signal-safe calls
/// on memory allocated before the fork. Exits with the errno of whatever fails, as that child does.
///
/// # Safety
/// Only in a child just forked from this process; `argv` and `envp` are null-terminated arrays of
/// pointers into `p`.
unsafe fn exec_child(p: &Prepared, argv: &[*const c_char], envp: &[*const c_char], fds: [RawFd; 3], flags: u32, max_fd: RawFd) -> ! {
    let [input, output, stderr] = fds;
    // Undo what the runtime set for itself: the VM's signals blocked, SIGPIPE ignored.
    let mut set: libc::sigset_t = std::mem::zeroed();
    libc::sigemptyset(&mut set);
    libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut());
    let mut dfl: libc::sigaction = std::mem::zeroed();
    dfl.sa_sigaction = libc::SIG_DFL;
    libc::sigaction(libc::SIGPIPE, &dfl, std::ptr::null_mut());
    libc::setsid();

    // Copied above the targets first, so placing one cannot overwrite another still to be placed.
    let high = |fd: RawFd| libc::fcntl(fd, libc::F_DUPFD, 10);
    let (i, o, e) = (high(input), high(output), high(stderr));
    if i < 0 || o < 0 || e < 0 {
        libc::_exit(last_errno());
    }
    // The fork copied every fd the VM has open. Drop them now rather than at execve, so a socket
    // or pipe the guest closes while this child waits for the Ack really closes.
    close_others([i, o, e], max_fd);

    let mut ack = [0u8; PROTO_LEN];
    if !read_exact(i, &mut ack) {
        libc::_exit(libc::EPIPE);
    }
    if let Some((dir, required)) = &p.dir {
        if libc::chdir(dir.as_ptr()) < 0 && *required {
            libc::_exit(last_errno());
        }
    }
    // Ports with nouse_stdio talk over fds 3 (from the VM) and 4 (to it).
    let (to_in, to_out) = if flags & FLAG_USE_STDIO != 0 { (0, 1) } else { (3, 4) };
    if (flags & FLAG_DO_WRITE != 0 && libc::dup2(i, to_in) < 0) || (flags & FLAG_DO_READ != 0 && libc::dup2(o, to_out) < 0) || libc::dup2(e, 2) < 0 {
        libc::_exit(last_errno());
    }
    libc::close(i);
    libc::close(o);
    libc::close(e);
    libc::execve(p.program.as_ptr(), argv.as_ptr(), envp.as_ptr());
    libc::_exit(last_errno())
}

/// Close every fd from 3 below `max_fd` except `keep`. Async-signal-safe.
///
/// # Safety
/// Only in a child between fork and exec: it closes fds other code may own.
unsafe fn close_others(mut keep: [RawFd; 3], max_fd: RawFd) {
    keep.sort_unstable();
    let mut lo = 3;
    for k in keep.into_iter().chain(std::iter::once(max_fd)) {
        if k > lo {
            close_range(lo, k - 1);
        }
        lo = lo.max(k + 1);
    }
}

/// Close the fds `lo..=hi`. Async-signal-safe.
///
/// # Safety
/// As for `close_others`.
unsafe fn close_range(lo: RawFd, hi: RawFd) {
    #[cfg(target_os = "linux")]
    if libc::syscall(libc::SYS_close_range, lo as libc::c_uint, hi as libc::c_uint, 0 as libc::c_uint) == 0 {
        return;
    }
    // Before Linux 5.9, and on macOS: one at a time.
    for fd in lo..=hi {
        libc::close(fd);
    }
}

/// One past the highest fd open now, for `close_others`: read before the fork, where listing a
/// directory is allowed. An fd opened after this is close-on-exec already (see `fds`).
fn open_max() -> RawFd {
    let dir = if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" };
    let listed = std::fs::read_dir(dir).ok().map(|d| d.filter_map(|e| e.ok()?.file_name().to_str()?.parse::<RawFd>().ok()).max().unwrap_or(2) + 1);
    listed.unwrap_or_else(|| match rustix::process::getrlimit(rustix::process::Resource::Nofile).current {
        Some(n) if n > 0 => n.min(1 << 16) as RawFd,
        _ => 1 << 16,
    })
}

fn null_terminated(v: &[CString]) -> Vec<*const c_char> {
    v.iter().map(|s| s.as_ptr()).chain(std::iter::once(std::ptr::null())).collect()
}

/// erl_child_setup's work for one Start request: `fds` are the child's stdin (which carries the
/// command block, then the VM's Ack), its stdout (where Go goes) and its stderr.
fn child_setup(k: Arc<Kernel>, port_id: u32, fds: [Arc<Entry>; 3]) {
    let _live = Live;
    // `fds` keeps these host fds open until the child has its own copies.
    let Ok(output) = fds[1].host_fd() else { return };
    let go = |pid: i32, e: i32| write_all(output, &proto(ACTION_GO, pid as u32, e));
    let (Ok(input), Ok(stderr)) = (fds[0].host_fd(), fds[2].host_fd()) else {
        go(-1, errno::BADF);
        return;
    };
    let Some(req) = read_request(input) else {
        go(-1, errno::IO);
        return;
    };
    let p = match prepare(&k, &req) {
        Ok(p) => p,
        Err(e) => {
            go(-1, e);
            return;
        },
    };
    let (argv, envp, max_fd) = (null_terminated(&p.argv), null_terminated(&p.env), open_max());
    let forked = {
        let _forking = crate::fds::forking();
        // SAFETY: the child runs only exec_child, which never returns.
        match unsafe { libc::fork() } {
            0 => unsafe { exec_child(&p, &argv, &envp, [input, output, stderr], req.flags, max_fd) },
            -1 => Err(errno::last()),
            pid => Ok(pid),
        }
    };
    let pid = match forked {
        Ok(pid) => pid,
        Err(e) => {
            go(-1, e);
            return;
        },
    };
    // As erl_child_setup: Go comes from the parent; the child execs once the VM acks it.
    go(pid, 0);
    drop(fds);
    let child = rustix::process::Pid::from_raw(pid);
    let status = loop {
        match rustix::process::waitpid(child, rustix::process::WaitOptions::empty()) {
            Ok(Some((_, st))) => break st.as_raw(),
            Err(rustix::io::Errno::INTR) => continue,
            // Cannot happen for our own child; report an exit rather than leave the port waiting.
            _ => break (libc::ECHILD & 0xff) << 8,
        }
    };
    if port_id != NO_PORT {
        if let Some(forker) = k.forker.get() {
            write_all(forker.as_raw_fd(), &proto(ACTION_SIGCHLD, port_id, status));
        }
    }
}

/// The `ampoule_os` imports for the spawn driver.
pub fn add(linker: &mut Linker<Ctx>) -> anyhow::Result<()> {
    let m = "ampoule_os";

    linker.func_wrap(m, "forker", |c: Caller<'_, Ctx>| -> i32 {
        let k = k(&c);
        ret((|| {
            let (rd, wr) = crate::fds::pipe()?;
            let r = k.insert(Kind::Pipe(rd))?;
            if k.forker.set(wr).is_err() {
                drop(k.remove(r));
                return Err(errno::BUSY);
            }
            Ok(r)
        })())
    })?;

    linker.func_wrap(m, "forker_write", |c: Caller<'_, Ctx>, _fd: i32, buf: i32, len: i32, fds: i32, nfds: i32| -> i32 {
        let kernel = c.data().k.clone();
        ret((|| {
            let (buf, fds) = (buf as u32, fds as u32);
            if len as usize != PROTO_LEN || nfds != 3 {
                return Err(errno::INVAL);
            }
            kernel.mem.ptr(buf, PROTO_LEN as u32)?;
            kernel.mem.array(fds, 3, 4)?;
            if kernel.mem.u32(buf)? != ACTION_START {
                return Err(errno::INVAL);
            }
            let port_id = kernel.mem.u32(buf + 4)?;
            // Held by entry, as a received SCM_RIGHTS fd would be: ERTS closes its numbers next.
            let mut held = Vec::with_capacity(3);
            for i in 0..3 {
                held.push(kernel.get(kernel.mem.u32(fds + i * 4)? as i32)?);
            }
            let held: [Arc<Entry>; 3] = held.try_into().map_err(|_| errno::INVAL)?;
            // Refused at once, with no thread: nothing may run, or too much already does.
            let refuse = |e: i32| {
                if let Ok(out) = held[1].host_fd() {
                    write_all(out, &proto(ACTION_GO, -1i32 as u32, e));
                }
                Ok(len)
            };
            if kernel.policy.exec.allowed.is_empty() {
                return refuse(errno::ACCES);
            }
            if CHILDREN.fetch_add(1, Ordering::Relaxed) >= MAX_CHILDREN {
                CHILDREN.fetch_sub(1, Ordering::Relaxed);
                return refuse(errno::AGAIN);
            }
            let (k, for_thread) = (kernel.clone(), held.clone());
            if std::thread::Builder::new().name("beam-child".into()).spawn(move || child_setup(k, port_id, for_thread)).is_err() {
                CHILDREN.fetch_sub(1, Ordering::Relaxed);
                return refuse(errno::AGAIN);
            }
            Ok(len)
        })())
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(flags: u32, strs: &[&str], env: &[&str], argv: Option<&[&str]>) -> Vec<u8> {
        let mut b = flags.to_be_bytes().to_vec();
        for s in strs {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        b.push(0);
        b.extend_from_slice(&(env.len() as u32).to_be_bytes());
        for s in env {
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        if let Some(argv) = argv {
            b.extend_from_slice(&(argv.len() as u32).to_be_bytes());
            for s in argv {
                b.extend_from_slice(s.as_bytes());
                b.push(0);
            }
        }
        b
    }

    #[test]
    fn parse_reads_a_spawn_executable_request() {
        let b = block(FLAG_USE_STDIO | FLAG_DO_READ, &["/usr/bin/git", "/app", "/tmp/w"], &["A=1", "B=2"], Some(&["git", "status"]));
        let r = parse(&b).unwrap();
        assert_eq!(
            r,
            Request {
                flags: FLAG_USE_STDIO | FLAG_DO_READ,
                cmd: b"/usr/bin/git".to_vec(),
                cwd: b"/app".to_vec(),
                wd: Some(b"/tmp/w".to_vec()),
                env: vec![b"A=1".to_vec(), b"B=2".to_vec()],
                argv: Some(vec![b"git".to_vec(), b"status".to_vec()]),
            }
        );
    }

    #[test]
    fn parse_reads_a_shell_request_without_wd_or_argv() {
        let r = parse(&block(0, &["exec echo hi", "/"], &[], None)).unwrap();
        assert_eq!((r.cmd.as_slice(), r.wd, r.argv), (&b"exec echo hi"[..], None, None));
    }

    #[test]
    fn parse_rejects_a_truncated_block() {
        let b = block(0, &["x", "/"], &["A=1"], None);
        assert_eq!(parse(&b[..b.len() - 2]), None);
    }

    #[test]
    fn proto_lays_out_go_as_the_guest_reads_it() {
        let p = proto(ACTION_GO, 1234, 0);
        assert_eq!((&p[0..4], &p[4..8]), (&2u32.to_le_bytes()[..], &1234u32.to_le_bytes()[..]));
    }

    #[test]
    fn resolve_finds_bare_names_on_path_and_skips_missing_ones() {
        let dir = crate::TestDir::new("exec");
        let bin = dir.0.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("tool"), b"#!/bin/sh\n").unwrap();
        std::fs::write(bin.join("data"), b"").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(bin.join("tool"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = bin.to_str().unwrap().to_string();
        let exec = Exec::resolve(&["tool".into(), "data".into(), "absent".into()], &path);
        let host = std::fs::canonicalize(bin.join("tool")).unwrap();
        assert_eq!(exec.allowed, vec![Allowed { guest: format!("{path}/tool"), host: host.clone(), dir: false }]);
        assert!(exec.program(&host) == Some(host.clone()) && exec.program(&bin.join("data")).is_none());
    }

    #[test]
    fn a_wildcard_runs_programs_directly_in_path_directories_wherever_they_link() {
        let dir = crate::TestDir::new("exec-star");
        let root = std::fs::canonicalize(&dir.0).unwrap();
        let (bin, cellar) = (root.join("bin"), root.join("cellar"));
        std::fs::create_dir_all(bin.join("sub")).unwrap();
        std::fs::create_dir_all(&cellar).unwrap();
        std::fs::write(cellar.join("git"), b"").unwrap();
        std::fs::write(bin.join("sub/deeper"), b"").unwrap();
        std::os::unix::fs::symlink(cellar.join("git"), bin.join("git")).unwrap();
        let exec = Exec::resolve(&["*".into()], bin.to_str().unwrap());
        assert_eq!(exec.program(&bin.join("git")), Some(bin.join("git")));
        assert_eq!((exec.program(&bin.join("sub/deeper")), exec.program(&cellar.join("git"))), (None, None));
    }

    #[test]
    fn mounts_leave_out_paths_the_runtime_owns() {
        let exec = Exec {
            allowed: vec![
                Allowed { guest: "/".into(), host: "/".into(), dir: true },
                Allowed { guest: "/app/bin".into(), host: "/x".into(), dir: true },
                Allowed { guest: "/usr/bin/git".into(), host: "/usr/bin/git".into(), dir: false },
            ],
        };
        let guests: Vec<String> = exec.mounts(&["/app", "/proc"]).into_iter().map(|m| m.guest).collect();
        assert_eq!(guests, vec!["/usr/bin/git".to_string()]);
    }
}
