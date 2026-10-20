//! wasi_snapshot_preview1, implemented over the shared `Kernel` so every wasm thread sees the
//! same fds and mounts. Only the calls beam.wasm imports are provided.

use std::ffi::CString;
use std::io::IsTerminal;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rustix::event::{PollFd, PollFlags};
use rustix::fs::{AtFlags, FileType, Mode, OFlags, SeekFrom, Timestamps};
use rustix::time::Timespec;
use wasmtime::{Caller, Linker};

use crate::errno::{self, check_len};
use crate::hostfs::{self, HostNode};
use crate::kernel::{normalize, Entry, Kernel, Kind, Node};
use crate::mem::R;
use crate::vfs::{self, RootFs};
use crate::{k, Ctx};

pub const FILETYPE_UNKNOWN: u8 = 0;
pub const FILETYPE_BLOCK: u8 = 1;
pub const FILETYPE_CHAR: u8 = 2;
pub const FILETYPE_DIR: u8 = 3;
pub const FILETYPE_REGULAR: u8 = 4;
pub const FILETYPE_SOCKET_DGRAM: u8 = 5;
pub const FILETYPE_SOCKET_STREAM: u8 = 6;
pub const FILETYPE_SYMLINK: u8 = 7;

const FDFLAGS_APPEND: u16 = 1;
const FDFLAGS_NONBLOCK: u16 = 4;

const OFLAGS_CREAT: i32 = 1;
const OFLAGS_DIRECTORY: i32 = 2;
const OFLAGS_EXCL: i32 = 4;
const OFLAGS_TRUNC: i32 = 8;

const RIGHTS_FD_READ: u64 = 1 << 1;
const RIGHTS_FD_WRITE: u64 = 1 << 6;
const RIGHTS_ALL: u64 = (1 << 30) - 1;
/// wasi-libc derives fcntl(F_GETFL)'s access mode from these two rights.
const RIGHTS_ALL_BUT_RW: u64 = RIGHTS_ALL & !(RIGHTS_FD_READ | RIGHTS_FD_WRITE);

const LOOKUP_SYMLINK_FOLLOW: i32 = 1;

const EVENTTYPE_CLOCK: u8 = 0;
const EVENTTYPE_FD_READ: u8 = 1;
const EVENTTYPE_FD_WRITE: u8 = 2;
const EVENT_HANGUP: u16 = 1;
const SUBCLOCK_ABSTIME: u16 = 1;

const FSTFLAGS_ATIM: i32 = 1;
const FSTFLAGS_ATIM_NOW: i32 = 2;
const FSTFLAGS_MTIM: i32 = 4;
const FSTFLAGS_MTIM_NOW: i32 = 8;

/// Embedded and virtual entries live on their own device so inode numbers can't collide with
/// host files.
const EMBEDDED_DEV: u64 = 0x5254_5752;
const VIRTUAL_DEV: u64 = 0x5254_5756;

fn code(r: R<()>) -> i32 {
    match r {
        Ok(()) => errno::SUCCESS,
        Err(e) => e,
    }
}

/// The guest path `path` names, relative to directory fd `dirfd`.
fn guest_path(k: &Kernel, dirfd: i32, path: u32, len: u32) -> R<String> {
    let rel = k.mem.str(path, len)?;
    let e = k.get(dirfd)?;
    let r = match &e.kind {
        Kind::Dir { guest } => Ok(normalize(&format!("{guest}/{rel}"))),
        _ => Err(errno::NOTDIR),
    };
    if crate::trace() {
        eprintln!("[trace] path dirfd={dirfd} rel={rel:?} -> {r:?}");
    }
    r
}

struct Stat {
    dev: u64,
    ino: u64,
    filetype: u8,
    nlink: u64,
    size: u64,
    atim: u64,
    mtim: u64,
    ctim: u64,
    /// Permission bits, which a WASI filestat cannot carry: only `path_mode` reports them.
    mode: u32,
}

fn ts_ns(sec: i64, nsec: i64) -> u64 {
    (sec as u64).wrapping_mul(1_000_000_000).wrapping_add(nsec as u64)
}

pub(crate) fn filetype_of(t: FileType) -> u8 {
    match t {
        FileType::Directory => FILETYPE_DIR,
        FileType::RegularFile => FILETYPE_REGULAR,
        FileType::Symlink => FILETYPE_SYMLINK,
        FileType::Socket => FILETYPE_SOCKET_STREAM,
        FileType::CharacterDevice => FILETYPE_CHAR,
        FileType::BlockDevice => FILETYPE_BLOCK,
        _ => FILETYPE_UNKNOWN,
    }
}

fn node_filetype(n: &vfs::Node) -> u8 {
    match n.kind {
        vfs::KIND_DIR => FILETYPE_DIR,
        vfs::KIND_SYMLINK => FILETYPE_SYMLINK,
        _ => FILETYPE_REGULAR,
    }
}

impl Stat {
    // The field types of a stat differ between macOS (libc) and Linux (the kernel's own struct):
    // these casts are no-ops on one platform and needed on the other.
    #[allow(clippy::unnecessary_cast)]
    fn of_stat(st: &rustix::fs::Stat) -> Stat {
        Stat {
            dev: st.st_dev as u64,
            ino: st.st_ino as u64,
            filetype: filetype_of(FileType::from_raw_mode(st.st_mode as _)),
            nlink: st.st_nlink as u64,
            size: st.st_size as u64,
            atim: ts_ns(st.st_atime as i64, st.st_atime_nsec as i64),
            mtim: ts_ns(st.st_mtime as i64, st.st_mtime_nsec as i64),
            ctim: ts_ns(st.st_ctime as i64, st.st_ctime_nsec as i64),
            mode: st.st_mode as u32 & 0o7777,
        }
    }

    fn of_node(n: &vfs::Node) -> Stat {
        Stat { dev: EMBEDDED_DEV, ino: n.ino, filetype: node_filetype(n), nlink: 1, size: n.size, atim: 0, mtim: 0, ctim: 0, mode: n.mode & 0o7777 }
    }

    fn virtual_dir(guest: &str) -> Stat {
        let ino = guest.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
        Stat { dev: VIRTUAL_DEV, ino, filetype: FILETYPE_DIR, nlink: 2, size: 0, atim: 0, mtim: 0, ctim: 0, mode: 0o555 }
    }

    fn of_fd(fd: BorrowedFd<'_>) -> R<Stat> {
        Ok(Stat::of_stat(&rustix::fs::fstat(fd).map_err(errno::rx)?))
    }

    fn write(&self, k: &Kernel, buf: u32) -> R<()> {
        let m = &k.mem;
        // The whole 64-byte filestat first, so the field offsets below cannot overflow.
        m.ptr(buf, 64)?;
        m.set_u64(buf, self.dev)?;
        m.set_u64(buf + 8, self.ino)?;
        m.set_u8(buf + 16, self.filetype)?;
        m.set_u64(buf + 24, self.nlink)?;
        m.set_u64(buf + 32, self.size)?;
        m.set_u64(buf + 40, self.atim)?;
        m.set_u64(buf + 48, self.mtim)?;
        m.set_u64(buf + 56, self.ctim)
    }
}

/// `fstatat` flags for a host node: a mount point is a path the runtime chose, followed as is;
/// below it the resolver has already followed what the guest asked to follow.
fn at_flags(n: &HostNode) -> AtFlags {
    if n.mount_root {
        AtFlags::empty()
    } else {
        AtFlags::SYMLINK_NOFOLLOW
    }
}

fn stat_host(n: &HostNode) -> R<Stat> {
    Ok(Stat::of_stat(&rustix::fs::statat(&n.dir, n.name.as_c_str(), at_flags(n)).map_err(errno::rx)?))
}

/// Stat a guest path through the mounts.
fn stat_path(k: &Kernel, guest: &str, follow: bool) -> R<Stat> {
    match k.resolve(guest, follow)? {
        Node::Host(n) => stat_host(&n),
        Node::Embedded { fs, path, .. } => {
            let n = fs.lookup(&path, follow).map(|(_, n)| n);
            n.map(Stat::of_node).ok_or(errno::NOENT)
        },
        Node::Proc { path, guest } => {
            if crate::proc::is_dir(&path) {
                Ok(Stat::virtual_dir(&guest))
            } else if crate::proc::read(k, &path).is_some() {
                Ok(Stat {
                    dev: VIRTUAL_DEV,
                    ino: Stat::virtual_dir(&guest).ino,
                    filetype: FILETYPE_REGULAR,
                    nlink: 1,
                    size: 0,
                    atim: 0,
                    mtim: 0,
                    ctim: 0,
                    mode: 0o444,
                })
            } else {
                Err(errno::NOENT)
            }
        },
        Node::Virtual { guest } => Ok(Stat::virtual_dir(&guest)),
        Node::Missing => Err(errno::NOENT),
    }
}

/// The permission bits of a guest path (`ampoule_os.path_mode`).
pub fn path_mode(k: &Kernel, guest: &str, follow: bool) -> R<u32> {
    stat_path(k, guest, follow).map(|s| s.mode)
}

/// The mode bits a guest may set. Never setuid or setgid: a guest running as root (in a
/// container) could otherwise leave a setuid-root program in a directory the host shares.
const CHMOD_BITS: u32 = 0o1777;

/// chmod on a guest path (`ampoule_os.chmod`), which follows symlinks within the guest's tree only.
pub fn chmod(k: &Kernel, guest: &str, mode: u32) -> R<()> {
    let n = host_for_write(k, guest, true, true)?;
    chmod_node(&n, rustix::fs::Mode::from_raw_mode((mode & CHMOD_BITS) as _))
}

/// The resolver has followed every symlink the guest asked to; one at the name now was swapped in
/// since, and is not followed, so the change cannot reach a host file outside the mounts. A mount
/// point is a path the runtime was given, followed as is.
#[cfg(target_os = "linux")]
fn chmod_node(n: &HostNode, mode: rustix::fs::Mode) -> R<()> {
    use rustix::fs::{FileType, OFlags};
    use std::os::fd::AsRawFd;
    let nofollow = if n.mount_root { OFlags::empty() } else { OFlags::NOFOLLOW };
    let fd = rustix::fs::openat(&n.dir, n.name.as_c_str(), OFlags::PATH | OFlags::CLOEXEC | nofollow, rustix::fs::Mode::empty()).map_err(errno::rx)?;
    if FileType::from_raw_mode(rustix::fs::fstat(&fd).map_err(errno::rx)?.st_mode) == FileType::Symlink {
        return Err(errno::LOOP);
    }
    // An O_PATH fd cannot be fchmod-ed; its /proc/self/fd entry names exactly the file opened.
    rustix::fs::chmod(format!("/proc/self/fd/{}", fd.as_raw_fd()), mode).map_err(errno::rx)
}

#[cfg(not(target_os = "linux"))]
fn chmod_node(n: &HostNode, mode: rustix::fs::Mode) -> R<()> {
    let flags = if n.mount_root { rustix::fs::AtFlags::empty() } else { rustix::fs::AtFlags::SYMLINK_NOFOLLOW };
    rustix::fs::chmodat(&n.dir, n.name.as_c_str(), mode, flags).map_err(errno::rx)
}

/// fchmod on a guest fd (`ampoule_os.fchmod`).
pub fn fchmod(k: &Kernel, fd: i32, mode: u32) -> R<()> {
    let e = k.get(fd)?;
    match &e.kind {
        Kind::Dir { guest } => chmod(k, guest, mode),
        Kind::File { ro: true, .. } | Kind::Mem { .. } => Err(errno::ROFS),
        // The runtime's own stdin/stdout/stderr are not the guest's to change.
        Kind::Stdio(_) => Err(errno::PERM),
        Kind::File { fd, .. } | Kind::Socket(fd) | Kind::Pipe(fd) => {
            rustix::fs::fchmod(fd, rustix::fs::Mode::from_raw_mode((mode & CHMOD_BITS) as _)).map_err(errno::rx)
        },
    }
}

/// The `ampoule_os` file calls WASI lacks: permission bits, chmod and fchmod.
pub fn add_ampoule_os(linker: &mut Linker<Ctx>) -> anyhow::Result<()> {
    let m = "ampoule_os";
    linker.func_wrap(m, "path_mode", |c: Caller<'_, Ctx>, path: i32, len: i32, follow: i32| -> i32 {
        let k = k(&c);
        crate::net::ret((|| path_mode(k, &normalize(&k.mem.str(path as u32, len as u32)?), follow != 0).map(|m| m as i32))())
    })?;
    linker.func_wrap(m, "chmod", |c: Caller<'_, Ctx>, path: i32, len: i32, mode: i32| -> i32 {
        let k = k(&c);
        crate::net::ret((|| chmod(k, &normalize(&k.mem.str(path as u32, len as u32)?), mode as u32).map(|()| 0))())
    })?;
    linker.func_wrap(m, "fchmod", |c: Caller<'_, Ctx>, fd: i32, mode: i32| -> i32 {
        let k = k(&c);
        crate::net::ret(fchmod(k, fd, mode as u32).map(|()| 0))
    })?;
    Ok(())
}

/// The host node a mutating call may act on. Read-only mounts, the embedded release, /proc and
/// virtual directories refuse with ROFS; a mount point itself can be written to (`allow_root`,
/// e.g. setting its times) but never removed, renamed or replaced (BUSY).
pub(crate) fn host_for_write(k: &Kernel, guest: &str, follow: bool, allow_root: bool) -> R<HostNode> {
    match k.resolve(guest, follow)? {
        Node::Host(n) if n.ro => Err(errno::ROFS),
        Node::Host(n) if n.mount_root && !allow_root => Err(errno::BUSY),
        Node::Host(n) => Ok(n),
        Node::Embedded { .. } | Node::Proc { .. } | Node::Virtual { .. } => Err(errno::ROFS),
        Node::Missing => Err(errno::NOENT),
    }
}

#[cfg(target_os = "linux")]
fn fill_random(buf: &mut [u8]) -> R<()> {
    let mut at = 0;
    while at < buf.len() {
        match rustix::rand::getrandom(&mut buf[at..], rustix::rand::GetRandomFlags::empty()) {
            Ok(r) => at += r,
            Err(rustix::io::Errno::INTR) => continue,
            Err(e) => return Err(errno::rx(e)),
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn fill_random(buf: &mut [u8]) -> R<()> {
    for chunk in buf.chunks_mut(256) {
        // SAFETY: a host buffer of at most 256 bytes, getentropy's limit. (rustix has no
        // getentropy, and macOS no getrandom.)
        errno::check(unsafe { libc::getentropy(chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) })?;
    }
    Ok(())
}

/// The access and modification times for utimensat/futimens from WASI's timestamps and fstflags.
fn utimens(atim: i64, mtim: i64, fst: i32) -> Timestamps {
    let spec = |set: i32, now: i32, t: i64| -> Timespec {
        if fst & now != 0 {
            Timespec { tv_sec: 0, tv_nsec: rustix::fs::UTIME_NOW as _ }
        } else if fst & set != 0 {
            Timespec { tv_sec: (t as u64 / 1_000_000_000) as i64, tv_nsec: (t as u64 % 1_000_000_000) as _ }
        } else {
            Timespec { tv_sec: 0, tv_nsec: rustix::fs::UTIME_OMIT as _ }
        }
    };
    Timestamps { last_access: spec(FSTFLAGS_ATIM, FSTFLAGS_ATIM_NOW, atim), last_modification: spec(FSTFLAGS_MTIM, FSTFLAGS_MTIM_NOW, mtim) }
}

/// fd_sync / fd_datasync: fsync host fds; embedded files and directories have nothing to flush.
fn sync(k: &Kernel, fd: i32) -> R<()> {
    let e = k.get(fd)?;
    match e.fd() {
        Ok(fd) => rustix::fs::fsync(fd).map_err(errno::rx),
        Err(_) => Ok(()),
    }
}

fn proc_exit(status: i32) {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    std::process::exit(status)
}

pub fn add(linker: &mut Linker<Ctx>) -> anyhow::Result<()> {
    let m = "wasi_snapshot_preview1";

    linker.func_wrap(m, "args_sizes_get", |c: Caller<'_, Ctx>, argc: i32, size: i32| -> i32 {
        let k = k(&c);
        let total: usize = k.args.iter().map(|a| a.len() + 1).sum();
        code(k.mem.set_u32(argc as u32, k.args.len() as u32).and_then(|_| k.mem.set_u32(size as u32, total as u32)))
    })?;
    linker.func_wrap(m, "args_get", |c: Caller<'_, Ctx>, argv: i32, buf: i32| -> i32 {
        let k = k(&c);
        code(write_strings(k, &k.args, argv as u32, buf as u32))
    })?;
    linker.func_wrap(m, "environ_sizes_get", |c: Caller<'_, Ctx>, count: i32, size: i32| -> i32 {
        let k = k(&c);
        let total: usize = k.env.iter().map(|a| a.len() + 1).sum();
        code(k.mem.set_u32(count as u32, k.env.len() as u32).and_then(|_| k.mem.set_u32(size as u32, total as u32)))
    })?;
    linker.func_wrap(m, "environ_get", |c: Caller<'_, Ctx>, env: i32, buf: i32| -> i32 {
        let k = k(&c);
        code(write_strings(k, &k.env, env as u32, buf as u32))
    })?;

    linker.func_wrap(m, "clock_time_get", |c: Caller<'_, Ctx>, id: i32, _precision: i64, out: i32| -> i32 {
        let k = k(&c);
        code(clock_now(k, id).and_then(|t| k.mem.set_u64(out as u32, t)))
    })?;

    linker.func_wrap(m, "random_get", |c: Caller<'_, Ctx>, buf: i32, len: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let (buf, len) = (buf as u32, len as u32);
            // Check the whole range before writing any of it; then fill it a chunk at a time.
            k.mem.ptr(buf, len)?;
            let mut chunk = [0u8; 256];
            let mut done = 0u32;
            while done < len {
                let n = (len - done).min(chunk.len() as u32) as usize;
                fill_random(&mut chunk[..n])?;
                k.mem.write(buf + done, &chunk[..n])?;
                done += n as u32;
            }
            Ok(())
        })())
    })?;

    linker.func_wrap(m, "sched_yield", |_c: Caller<'_, Ctx>| -> i32 {
        std::thread::yield_now();
        errno::SUCCESS
    })?;

    linker.func_wrap(m, "proc_exit", |_c: Caller<'_, Ctx>, status: i32| proc_exit(status))?;

    linker.func_wrap(m, "fd_advise", |_c: Caller<'_, Ctx>, _fd: i32, _off: i64, _len: i64, _advice: i32| -> i32 { errno::SUCCESS })?;

    linker.func_wrap(m, "fd_close", |c: Caller<'_, Ctx>, fd: i32| -> i32 { code(k(&c).remove(fd).map(|_| ())) })?;

    linker.func_wrap(m, "fd_fdstat_get", |c: Caller<'_, Ctx>, fd: i32, buf: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            let (ft, flags, rights) = match (&e.kind, e.fd()) {
                (Kind::Mem { .. }, _) => (FILETYPE_REGULAR, 0, RIGHTS_ALL_BUT_RW | RIGHTS_FD_READ),
                (Kind::Dir { .. }, _) => (FILETYPE_DIR, 0, RIGHTS_ALL),
                (_, Ok(fd)) => {
                    let mut ft = Stat::of_fd(fd)?.filetype;
                    if e.is_socket() && crate::net::is_datagram(fd) {
                        ft = FILETYPE_SOCKET_DGRAM;
                    }
                    let fl = rustix::fs::fcntl_getfl(fd).map_err(errno::rx)?;
                    let mut flags = 0u16;
                    if fl.contains(OFlags::NONBLOCK) {
                        flags |= FDFLAGS_NONBLOCK;
                    }
                    if fl.contains(OFlags::APPEND) {
                        flags |= FDFLAGS_APPEND;
                    }
                    let rw = match fl & OFlags::RWMODE {
                        m if m == OFlags::RDONLY => RIGHTS_FD_READ,
                        m if m == OFlags::WRONLY => RIGHTS_FD_WRITE,
                        _ => RIGHTS_FD_READ | RIGHTS_FD_WRITE,
                    };
                    (ft, flags, RIGHTS_ALL_BUT_RW | rw)
                },
                (_, Err(e)) => return Err(e),
            };
            let buf = buf as u32;
            k.mem.ptr(buf, 24)?;
            k.mem.set_u8(buf, ft)?;
            k.mem.set_u16(buf + 2, flags)?;
            k.mem.set_u64(buf + 8, rights)?;
            k.mem.set_u64(buf + 16, rights)
        })())
    })?;

    linker.func_wrap(m, "fd_fdstat_set_flags", |c: Caller<'_, Ctx>, fd: i32, flags: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            let Ok(fd) = e.fd() else { return Ok(()) };
            let mut fl = rustix::fs::fcntl_getfl(fd).map_err(errno::rx)?;
            fl.set(OFlags::NONBLOCK, flags as u16 & FDFLAGS_NONBLOCK != 0);
            fl.set(OFlags::APPEND, flags as u16 & FDFLAGS_APPEND != 0);
            rustix::fs::fcntl_setfl(fd, fl).map_err(errno::rx)
        })())
    })?;

    linker.func_wrap(m, "fd_filestat_get", |c: Caller<'_, Ctx>, fd: i32, buf: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            let st = match &e.kind {
                Kind::Mem { data, ino, .. } => {
                    Stat { dev: EMBEDDED_DEV, ino: *ino, filetype: FILETYPE_REGULAR, nlink: 1, size: data.len() as u64, atim: 0, mtim: 0, ctim: 0, mode: 0o444 }
                },
                Kind::Dir { guest } => stat_path(k, guest, true)?,
                _ => Stat::of_fd(e.fd()?)?,
            };
            st.write(k, buf as u32)
        })())
    })?;

    linker.func_wrap(m, "fd_filestat_set_size", |c: Caller<'_, Ctx>, fd: i32, size: i64| -> i32 {
        let k = k(&c);
        code((|| match &k.get(fd)?.kind {
            Kind::Mem { .. } => Err(errno::ROFS),
            Kind::Dir { .. } => Err(errno::ISDIR),
            Kind::File { ro: true, .. } => Err(errno::ROFS),
            Kind::File { fd, .. } => rustix::fs::ftruncate(fd, u64::try_from(size).map_err(|_| errno::INVAL)?).map_err(errno::rx),
            _ => Err(errno::INVAL),
        })())
    })?;

    linker.func_wrap(m, "fd_read", |c: Caller<'_, Ctx>, fd: i32, iovs: i32, n: i32, out: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            let read = match &e.kind {
                Kind::Mem { data, pos, .. } => {
                    let mut pos = pos.lock().unwrap();
                    let r = mem_read(k, data, *pos, iovs as u32, n as u32)?;
                    *pos += r as u64;
                    r
                },
                Kind::Dir { .. } => return Err(errno::ISDIR),
                _ => {
                    let v = k.mem.iovecs(iovs as u32, n as u32)?;
                    // SAFETY: `v` holds bounds-checked guest buffers the kernel fills in place (no
                    // Rust slice over guest memory, see `mem`); `e` keeps the fd open.
                    check_len(unsafe { libc::readv(e.host_fd()?, v.as_ptr(), v.len() as libc::c_int) })?
                },
            };
            k.mem.set_u32(out as u32, read as u32)
        })())
    })?;

    linker.func_wrap(m, "fd_pread", |c: Caller<'_, Ctx>, fd: i32, iovs: i32, n: i32, offset: i64, out: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            let total = match &e.kind {
                Kind::Mem { data, .. } => mem_read(k, data, offset as u64, iovs as u32, n as u32)?,
                Kind::Dir { .. } => return Err(errno::ISDIR),
                _ => {
                    let raw = e.host_fd()?;
                    let v = k.mem.iovecs(iovs as u32, n as u32)?;
                    let mut total = 0usize;
                    let mut off = offset;
                    for iov in &v {
                        // SAFETY: as for readv: a bounds-checked guest buffer; `e` keeps `raw` open.
                        let r = check_len(unsafe { libc::pread(raw, iov.iov_base, iov.iov_len, off as libc::off_t) })?;
                        total += r;
                        off += r as i64;
                        if r < iov.iov_len {
                            break;
                        }
                    }
                    total
                },
            };
            k.mem.set_u32(out as u32, total as u32)
        })())
    })?;

    linker.func_wrap(m, "fd_write", |c: Caller<'_, Ctx>, fd: i32, iovs: i32, n: i32, out: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            let raw = e.host_fd()?;
            let v = k.mem.iovecs(iovs as u32, n as u32)?;
            // SAFETY: `v` holds bounds-checked guest buffers the kernel reads in place; `e` keeps
            // `raw` open.
            let r = check_len(unsafe { libc::writev(raw, v.as_ptr(), v.len() as libc::c_int) })?;
            k.mem.set_u32(out as u32, r as u32)
        })())
    })?;

    linker.func_wrap(m, "fd_seek", |c: Caller<'_, Ctx>, fd: i32, offset: i64, whence: i32, out: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            let new = match &e.kind {
                Kind::Mem { data, pos, .. } => {
                    let mut pos = pos.lock().unwrap();
                    let base = match whence {
                        0 => 0i64,
                        1 => *pos as i64,
                        2 => data.len() as i64,
                        _ => return Err(errno::INVAL),
                    };
                    let p = base.checked_add(offset).filter(|p| *p >= 0).ok_or(errno::INVAL)?;
                    *pos = p as u64;
                    p as u64
                },
                Kind::Dir { .. } => return Err(errno::ISDIR),
                _ => {
                    let to = match whence {
                        0 => SeekFrom::Start(u64::try_from(offset).map_err(|_| errno::INVAL)?),
                        1 => SeekFrom::Current(offset),
                        2 => SeekFrom::End(offset),
                        _ => return Err(errno::INVAL),
                    };
                    rustix::fs::seek(e.fd()?, to).map_err(errno::rx)?
                },
            };
            k.mem.set_u64(out as u32, new)
        })())
    })?;

    linker.func_wrap(m, "fd_tell", |c: Caller<'_, Ctx>, fd: i32, out: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            let p = match &e.kind {
                Kind::Mem { pos, .. } => *pos.lock().unwrap(),
                _ => rustix::fs::tell(e.fd()?).map_err(errno::rx)?,
            };
            k.mem.set_u64(out as u32, p)
        })())
    })?;

    linker.func_wrap(m, "fd_sync", |c: Caller<'_, Ctx>, fd: i32| -> i32 { code(sync(k(&c), fd)) })?;

    linker.func_wrap(m, "fd_prestat_get", |c: Caller<'_, Ctx>, fd: i32, buf: i32| -> i32 {
        let k = k(&c);
        code((|| {
            if fd != Kernel::PREOPEN_FD {
                return Err(errno::BADF);
            }
            k.mem.ptr(buf as u32, 8)?;
            k.mem.set_u8(buf as u32, 0)?;
            k.mem.set_u32(buf as u32 + 4, 1)
        })())
    })?;

    linker.func_wrap(m, "fd_prestat_dir_name", |c: Caller<'_, Ctx>, fd: i32, path: i32, len: i32| -> i32 {
        let k = k(&c);
        code((|| {
            if fd != Kernel::PREOPEN_FD || len < 1 {
                return Err(errno::BADF);
            }
            k.mem.write(path as u32, b"/")
        })())
    })?;

    linker.func_wrap(m, "fd_readdir", |c: Caller<'_, Ctx>, fd: i32, buf: i32, len: i32, cookie: i64, out: i32| -> i32 {
        let k = k(&c);
        code(readdir(k, fd, buf as u32, len as u32, cookie as u64, out as u32))
    })?;

    linker.func_wrap(
        m,
        "path_open",
        |c: Caller<'_, Ctx>, dirfd: i32, dirflags: i32, path: i32, path_len: i32, oflags: i32, rights: i64, _inh: i64, fdflags: i32, out: i32| -> i32 {
            let k = k(&c);
            code(path_open(k, dirfd, dirflags, path as u32, path_len as u32, oflags, rights as u64, fdflags as u16, out as u32))
        },
    )?;

    linker.func_wrap(m, "path_filestat_get", |c: Caller<'_, Ctx>, dirfd: i32, flags: i32, path: i32, len: i32, buf: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let g = guest_path(k, dirfd, path as u32, len as u32)?;
            stat_path(k, &g, flags & LOOKUP_SYMLINK_FOLLOW != 0)?.write(k, buf as u32)
        })())
    })?;

    linker.func_wrap(
        m,
        "path_filestat_set_times",
        |c: Caller<'_, Ctx>, dirfd: i32, flags: i32, path: i32, len: i32, atim: i64, mtim: i64, fst: i32| -> i32 {
            let k = k(&c);
            code((|| {
                let g = guest_path(k, dirfd, path as u32, len as u32)?;
                let n = host_for_write(k, &g, flags & LOOKUP_SYMLINK_FOLLOW != 0, true)?;
                let times = utimens(atim, mtim, fst);
                rustix::fs::utimensat(&n.dir, n.name.as_c_str(), &times, at_flags(&n)).map_err(errno::rx)
            })())
        },
    )?;

    linker.func_wrap(m, "path_create_directory", |c: Caller<'_, Ctx>, dirfd: i32, path: i32, len: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let g = guest_path(k, dirfd, path as u32, len as u32)?;
            // EEXIST comes before EROFS and EBUSY, as on Linux: mkdir -p walks over mount points,
            // the release and virtual directories, and stops only at EEXIST.
            if stat_path(k, &g, false).is_ok() {
                return Err(errno::EXIST);
            }
            let n = host_for_write(k, &g, false, false)?;
            rustix::fs::mkdirat(&n.dir, n.name.as_c_str(), Mode::from_raw_mode(0o777)).map_err(errno::rx)
        })())
    })?;

    linker.func_wrap(m, "path_remove_directory", |c: Caller<'_, Ctx>, dirfd: i32, path: i32, len: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let n = host_for_write(k, &guest_path(k, dirfd, path as u32, len as u32)?, false, false)?;
            rustix::fs::unlinkat(&n.dir, n.name.as_c_str(), AtFlags::REMOVEDIR).map_err(errno::rx)
        })())
    })?;

    linker.func_wrap(m, "path_unlink_file", |c: Caller<'_, Ctx>, dirfd: i32, path: i32, len: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let n = host_for_write(k, &guest_path(k, dirfd, path as u32, len as u32)?, false, false)?;
            rustix::fs::unlinkat(&n.dir, n.name.as_c_str(), AtFlags::empty()).map_err(errno::rx)
        })())
    })?;

    linker.func_wrap(m, "path_rename", |c: Caller<'_, Ctx>, fd: i32, old: i32, old_len: i32, new_fd: i32, new: i32, new_len: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let a = host_for_write(k, &guest_path(k, fd, old as u32, old_len as u32)?, false, false)?;
            let b = host_for_write(k, &guest_path(k, new_fd, new as u32, new_len as u32)?, false, false)?;
            rustix::fs::renameat(&a.dir, a.name.as_c_str(), &b.dir, b.name.as_c_str()).map_err(errno::rx)
        })())
    })?;

    linker.func_wrap(m, "path_symlink", |c: Caller<'_, Ctx>, old: i32, old_len: i32, fd: i32, new: i32, new_len: i32| -> i32 {
        let k = k(&c);
        code((|| {
            // The target is stored as given: it is only ever resolved inside the guest's namespace.
            let target = CString::new(k.mem.read(old as u32, old_len as u32)?).map_err(|_| errno::INVAL)?;
            let n = host_for_write(k, &guest_path(k, fd, new as u32, new_len as u32)?, false, false)?;
            rustix::fs::symlinkat(target.as_c_str(), &n.dir, n.name.as_c_str()).map_err(errno::rx)
        })())
    })?;

    linker.func_wrap(m, "path_link", |c: Caller<'_, Ctx>, old_fd: i32, flags: i32, old: i32, old_len: i32, new_fd: i32, new: i32, new_len: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let a = host_for_write(k, &guest_path(k, old_fd, old as u32, old_len as u32)?, flags & LOOKUP_SYMLINK_FOLLOW != 0, false)?;
            let b = host_for_write(k, &guest_path(k, new_fd, new as u32, new_len as u32)?, false, false)?;
            rustix::fs::linkat(&a.dir, a.name.as_c_str(), &b.dir, b.name.as_c_str(), AtFlags::empty()).map_err(errno::rx)
        })())
    })?;

    linker.func_wrap(m, "path_readlink", |c: Caller<'_, Ctx>, fd: i32, path: i32, len: i32, buf: i32, buf_len: i32, out: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let g = guest_path(k, fd, path as u32, len as u32)?;
            let target = match k.resolve(&g, false)? {
                Node::Host(n) => hostfs::readlink_at(&n.dir, &n.name)?,
                Node::Embedded { fs, path, .. } => fs.lookup(&path, false).and_then(|(_, n)| fs.readlink(n)).ok_or(errno::INVAL)?,
                Node::Proc { .. } | Node::Virtual { .. } => return Err(errno::INVAL),
                Node::Missing => return Err(errno::NOENT),
            };
            let n = target.len().min(buf_len as u32 as usize);
            k.mem.write(buf as u32, &target[..n])?;
            k.mem.set_u32(out as u32, n as u32)
        })())
    })?;

    linker.func_wrap(m, "clock_res_get", |c: Caller<'_, Ctx>, id: i32, out: i32| -> i32 {
        let k = k(&c);
        code(if (0..=3).contains(&id) { k.mem.set_u64(out as u32, 1) } else { Err(errno::INVAL) })
    })?;

    linker.func_wrap(m, "proc_raise", |_c: Caller<'_, Ctx>, sig: i32| -> i32 {
        use std::io::Write;
        let _ = std::io::stdout().flush();
        eprintln!("ampoule: the app raised signal {sig}");
        std::process::exit(128 + sig)
    })?;

    linker.func_wrap(m, "fd_pwrite", |c: Caller<'_, Ctx>, fd: i32, iovs: i32, n: i32, offset: i64, out: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            let raw = e.host_fd()?;
            let v = k.mem.iovecs(iovs as u32, n as u32)?;
            let mut total = 0usize;
            let mut off = offset;
            for iov in &v {
                // SAFETY: as for writev.
                let r = check_len(unsafe { libc::pwrite(raw, iov.iov_base, iov.iov_len, off as libc::off_t) })?;
                total += r;
                off += r as i64;
                if r < iov.iov_len {
                    break;
                }
            }
            k.mem.set_u32(out as u32, total as u32)
        })())
    })?;

    linker.func_wrap(m, "fd_datasync", |c: Caller<'_, Ctx>, fd: i32| -> i32 { code(sync(k(&c), fd)) })?;

    linker.func_wrap(m, "fd_allocate", |c: Caller<'_, Ctx>, fd: i32, offset: i64, len: i64| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            if matches!(e.kind, Kind::File { ro: true, .. }) {
                return Err(errno::ROFS);
            }
            let fd = e.fd()?;
            let st = Stat::of_fd(fd)?;
            let want = offset.checked_add(len).filter(|w| *w >= 0).ok_or(errno::INVAL)? as u64;
            if want > st.size {
                rustix::fs::ftruncate(fd, want).map_err(errno::rx)?;
            }
            Ok(())
        })())
    })?;

    linker.func_wrap(m, "fd_filestat_set_times", |c: Caller<'_, Ctx>, fd: i32, atim: i64, mtim: i64, fst: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            if matches!(e.kind, Kind::File { ro: true, .. }) {
                return Err(errno::ROFS);
            }
            rustix::fs::futimens(e.fd()?, &utimens(atim, mtim, fst)).map_err(errno::rx)
        })())
    })?;

    linker.func_wrap(m, "fd_renumber", |c: Caller<'_, Ctx>, from: i32, to: i32| -> i32 { code(k(&c).renumber(from, to)) })?;

    linker.func_wrap(m, "fd_fdstat_set_rights", |_c: Caller<'_, Ctx>, _fd: i32, _base: i64, _inh: i64| -> i32 { errno::SUCCESS })?;

    // WASI's own socket calls (Rust's std::net on wasip1 uses them), on the same host sockets.
    linker.func_wrap(m, "sock_accept", |c: Caller<'_, Ctx>, fd: i32, flags: i32, out: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            if !e.is_socket() {
                return Err(errno::NOTSOCK);
            }
            let (conn, _) = crate::fds::accept(e.fd()?, false)?;
            let g = crate::net::adopt_socket(k, conn, flags as u16 & FDFLAGS_NONBLOCK != 0)?;
            k.mem.set_u32(out as u32, g as u32)
        })())
    })?;

    linker.func_wrap(m, "sock_recv", |c: Caller<'_, Ctx>, fd: i32, iovs: i32, n: i32, flags: i32, out_len: i32, out_flags: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            let raw = e.host_fd()?;
            let v = k.mem.iovecs(iovs as u32, n as u32)?;
            // SAFETY: all-zero is a valid msghdr.
            let mut hdr: libc::msghdr = unsafe { std::mem::zeroed() };
            hdr.msg_iov = v.as_ptr() as *mut libc::iovec;
            hdr.msg_iovlen = v.len() as _;
            let mut hf = 0;
            if flags & 1 != 0 {
                hf |= libc::MSG_PEEK;
            }
            if flags & 2 != 0 {
                hf |= libc::MSG_WAITALL;
            }
            // SAFETY: `hdr` points at bounds-checked guest buffers the kernel fills in place; `e`
            // keeps `raw` open.
            let r = check_len(unsafe { libc::recvmsg(raw, &mut hdr, hf) })?;
            k.mem.set_u32(out_len as u32, r as u32)?;
            k.mem.set_u16(out_flags as u32, if hdr.msg_flags & libc::MSG_TRUNC != 0 { 1 } else { 0 })
        })())
    })?;

    linker.func_wrap(m, "sock_send", |c: Caller<'_, Ctx>, fd: i32, iovs: i32, n: i32, _flags: i32, out_len: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            let raw = e.host_fd()?;
            let v = k.mem.iovecs(iovs as u32, n as u32)?;
            // SAFETY: all-zero is a valid msghdr.
            let mut hdr: libc::msghdr = unsafe { std::mem::zeroed() };
            hdr.msg_iov = v.as_ptr() as *mut libc::iovec;
            hdr.msg_iovlen = v.len() as _;
            #[cfg(target_os = "linux")]
            let hf = libc::MSG_NOSIGNAL;
            #[cfg(not(target_os = "linux"))]
            let hf = 0;
            // SAFETY: `hdr` points at bounds-checked guest buffers the kernel reads in place; `e`
            // keeps `raw` open.
            let r = check_len(unsafe { libc::sendmsg(raw, &hdr, hf) })?;
            k.mem.set_u32(out_len as u32, r as u32)
        })())
    })?;

    linker.func_wrap(m, "sock_shutdown", |c: Caller<'_, Ctx>, fd: i32, how: i32| -> i32 {
        let k = k(&c);
        code((|| {
            let e = k.get(fd)?;
            rustix::net::shutdown(e.fd()?, crate::net::shutdown_how(how)?).map_err(errno::rx)
        })())
    })?;

    linker.func_wrap(m, "poll_oneoff", |c: Caller<'_, Ctx>, subs: i32, events: i32, n: i32, out: i32| -> i32 {
        let k = k(&c);
        code(poll_oneoff(k, subs as u32, events as u32, n as u32, out as u32))
    })?;

    Ok(())
}

/// Copy `data[pos..]` into the guest iovecs; returns the bytes copied.
fn mem_read(k: &Kernel, data: &[u8], pos: u64, iovs: u32, n: u32) -> R<usize> {
    k.mem.array(iovs, n, 8)?;
    let mut at = usize::try_from(pos).unwrap_or(usize::MAX).min(data.len());
    let start = at;
    for i in 0..n {
        let buf = k.mem.u32(iovs + i * 8)?;
        let len = k.mem.u32(iovs + i * 8 + 4)? as usize;
        let take = len.min(data.len() - at);
        k.mem.write(buf, &data[at..at + take])?;
        at += take;
        if take < len {
            break;
        }
    }
    Ok(at - start)
}

fn write_strings(k: &Kernel, items: &[Vec<u8>], ptrs: u32, buf: u32) -> R<()> {
    let total: usize = items.iter().map(|s| s.len() + 1).sum();
    k.mem.array(ptrs, u32::try_from(items.len()).map_err(|_| errno::FAULT)?, 4)?;
    k.mem.ptr(buf, u32::try_from(total).map_err(|_| errno::FAULT)?)?;
    let mut at = buf;
    for (i, s) in items.iter().enumerate() {
        k.mem.set_u32(ptrs + i as u32 * 4, at)?;
        k.mem.write(at, s)?;
        k.mem.set_u8(at + s.len() as u32, 0)?;
        at += s.len() as u32 + 1;
    }
    Ok(())
}

fn clock_now(k: &Kernel, id: i32) -> R<u64> {
    match id {
        0 => Ok(SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| errno::INVAL)?.as_nanos() as u64),
        1 => Ok(k.now_mono_ns()),
        2 | 3 => {
            let clock = if id == 2 { rustix::time::ClockId::ProcessCPUTime } else { rustix::time::ClockId::ThreadCPUTime };
            let ts = rustix::time::clock_gettime(clock);
            Ok(ts_ns(ts.tv_sec, ts.tv_nsec as i64))
        },
        _ => Err(errno::INVAL),
    }
}

#[allow(clippy::too_many_arguments)]
fn path_open(k: &Kernel, dirfd: i32, dirflags: i32, path: u32, path_len: u32, oflags: i32, rights: u64, fdflags: u16, out: u32) -> R<()> {
    let guest = guest_path(k, dirfd, path, path_len)?;
    let follow = dirflags & LOOKUP_SYMLINK_FOLLOW != 0;
    let wants_write = rights & RIGHTS_FD_WRITE != 0 || oflags & (OFLAGS_CREAT | OFLAGS_TRUNC) != 0;
    // Directories are recorded by their resolved guest path, so one opened through a symlink
    // lists its target.
    let kind = match k.resolve(&guest, follow)? {
        Node::Virtual { guest } => {
            if oflags & OFLAGS_CREAT != 0 && oflags & OFLAGS_EXCL != 0 {
                return Err(errno::EXIST);
            }
            Kind::Dir { guest }
        },
        Node::Missing => return Err(if oflags & OFLAGS_CREAT != 0 { errno::ROFS } else { errno::NOENT }),
        Node::Embedded { fs, path, guest } => open_embedded(fs, &guest, &path, follow, oflags, wants_write)?,
        Node::Proc { path, guest } => {
            if crate::proc::is_dir(&path) {
                Kind::Dir { guest }
            } else if wants_write {
                return Err(errno::ACCES);
            } else {
                let data = crate::proc::read(k, &path).ok_or(errno::NOENT)?;
                Kind::Mem { data: Arc::new(data), pos: Mutex::new(0), ino: 0 }
            }
        },
        Node::Host(n) => {
            let st = stat_host(&n);
            let is_dir = st.as_ref().is_ok_and(|s| s.filetype == FILETYPE_DIR);
            if oflags & OFLAGS_DIRECTORY != 0 && !is_dir {
                return Err(if st.is_ok() { errno::NOTDIR } else { errno::NOENT });
            }
            if is_dir {
                if oflags & (OFLAGS_CREAT | OFLAGS_EXCL) == (OFLAGS_CREAT | OFLAGS_EXCL) {
                    return Err(errno::EXIST);
                }
                Kind::Dir { guest: n.guest }
            } else {
                if n.ro && wants_write {
                    return Err(errno::ROFS);
                }
                Kind::File { fd: open_host(&n, oflags, rights, fdflags)?, ro: n.ro }
            }
        },
    };
    let fd = k.insert(kind)?;
    k.mem.set_u32(out, fd as u32)
}

fn open_embedded(fs: &Arc<RootFs>, guest: &str, path: &str, follow: bool, oflags: i32, wants_write: bool) -> R<Kind> {
    let found = fs.lookup(path, follow);
    let Some((_, node)) = found else {
        return Err(if oflags & OFLAGS_CREAT != 0 { errno::ROFS } else { errno::NOENT });
    };
    if oflags & OFLAGS_CREAT != 0 && oflags & OFLAGS_EXCL != 0 {
        return Err(errno::EXIST);
    }
    match node.kind {
        vfs::KIND_DIR => Ok(Kind::Dir { guest: guest.to_string() }),
        _ if oflags & OFLAGS_DIRECTORY != 0 => Err(errno::NOTDIR),
        _ if wants_write => Err(errno::ROFS),
        vfs::KIND_SYMLINK => Err(errno::LOOP),
        _ => {
            let data = fs.read(node).map_err(|_| errno::IO)?;
            Ok(Kind::Mem { data: Arc::new(data), pos: Mutex::new(0), ino: node.ino })
        },
    }
}

/// Open a host file: `openat` in its resolved parent, never following a final symlink below a
/// mount point (the resolver already followed it when the guest asked to).
fn open_host(n: &HostNode, oflags: i32, rights: u64, fdflags: u16) -> R<OwnedFd> {
    let mut flags = match (rights & RIGHTS_FD_READ != 0, rights & RIGHTS_FD_WRITE != 0) {
        (_, false) => OFlags::RDONLY,
        (false, true) => OFlags::WRONLY,
        (true, true) => OFlags::RDWR,
    };
    flags.set(OFlags::CREATE, oflags & OFLAGS_CREAT != 0);
    flags.set(OFlags::EXCL, oflags & OFLAGS_EXCL != 0);
    flags.set(OFlags::TRUNC, oflags & OFLAGS_TRUNC != 0);
    flags.set(OFlags::APPEND, fdflags & FDFLAGS_APPEND != 0);
    flags.set(OFlags::NONBLOCK, fdflags & FDFLAGS_NONBLOCK != 0);
    flags.set(OFlags::NOFOLLOW, !n.mount_root);
    rustix::fs::openat(&n.dir, n.name.as_c_str(), flags | OFlags::CLOEXEC, Mode::from_raw_mode(0o644)).map_err(errno::rx)
}

fn readdir(k: &Kernel, fd: i32, buf: u32, len: u32, cookie: u64, out: u32) -> R<()> {
    let e = k.get(fd)?;
    let Kind::Dir { guest } = &e.kind else { return Err(errno::NOTDIR) };
    let mut entries: Vec<(Vec<u8>, u64, u8)> = vec![(b".".to_vec(), 0, FILETYPE_DIR), (b"..".to_vec(), 0, FILETYPE_DIR)];
    k.mem.ptr(buf, len)?;
    let mut rest: Vec<(Vec<u8>, u64, u8)> = match k.resolve(guest, true)? {
        Node::Host(n) => hostfs::list(&n)?,
        Node::Embedded { fs, path, .. } => fs.list(&path).into_iter().map(|(name, n)| (name.into_bytes(), n.ino, node_filetype(n))).collect(),
        Node::Proc { path, .. } => crate::proc::list(k, &path)
            .ok_or(errno::NOTDIR)?
            .into_iter()
            .map(|(name, e)| {
                let ft = match e {
                    crate::proc::Entry::Dir => FILETYPE_DIR,
                    crate::proc::Entry::File => FILETYPE_REGULAR,
                    crate::proc::Entry::Link => FILETYPE_SYMLINK,
                };
                (name.into_bytes(), 0, ft)
            })
            .collect(),
        Node::Virtual { .. } => Vec::new(),
        Node::Missing => return Err(errno::NOENT),
    };
    for name in k.mount_children(guest) {
        if !rest.iter().any(|(n, _, _)| n == name.as_bytes()) {
            let child = if guest == "/" { format!("/{name}") } else { format!("{guest}/{name}") };
            let ft = stat_path(k, &child, true).map(|s| s.filetype).unwrap_or(FILETYPE_DIR);
            rest.push((name.into_bytes(), 0, ft));
        }
    }
    rest.sort();
    entries.extend(rest);

    // wasi-libc's readdir skips entries whose inode is 0: give synthetic entries one.
    for (name, ino, _) in entries.iter_mut() {
        if *ino == 0 {
            *ino = name.iter().fold(0x811c_9dc5u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100_0000_01b3)) | 1;
        }
    }

    let mut used = 0u32;
    for (i, (name, ino, ft)) in entries.iter().enumerate().skip(cookie as usize) {
        let mut d = Vec::with_capacity(24 + name.len());
        d.extend_from_slice(&(i as u64 + 1).to_le_bytes());
        d.extend_from_slice(&ino.to_le_bytes());
        d.extend_from_slice(&(name.len() as u32).to_le_bytes());
        d.push(*ft);
        d.extend_from_slice(&[0, 0, 0]);
        d.extend_from_slice(name);
        let room = (len - used) as usize;
        let n = d.len().min(room);
        k.mem.write(buf + used, &d[..n])?;
        used += n as u32;
        if n < d.len() {
            break;
        }
    }
    k.mem.set_u32(out, used)
}

struct Sub {
    userdata: u64,
    tag: u8,
    fd: i32,
    deadline: Option<Duration>,
}

fn poll_oneoff(k: &Kernel, subs: u32, events: u32, n: u32, out: u32) -> R<()> {
    let m = &k.mem;
    if n == 0 {
        // poll(NULL, 0, -1) / select(0, ...) with no timeout: ERTS's main thread parks here for
        // the life of the VM.
        loop {
            std::thread::park();
        }
    }
    m.array(subs, n, 48)?;
    m.array(events, n, 32)?;
    let mut list = Vec::with_capacity(n as usize);
    for i in 0..n {
        let s = subs + i * 48;
        let userdata = m.u64(s)?;
        let tag = m.u8(s + 8)?;
        let (fd, deadline) = match tag {
            EVENTTYPE_CLOCK => {
                let id = m.u32(s + 16)? as i32;
                let timeout = m.u64(s + 24)?;
                let flags = m.u16(s + 40)?;
                let wait = if flags & SUBCLOCK_ABSTIME != 0 { timeout.saturating_sub(clock_now(k, id)?) } else { timeout };
                (-1, Some(Duration::from_nanos(wait)))
            },
            EVENTTYPE_FD_READ | EVENTTYPE_FD_WRITE => (m.u32(s + 16)? as i32, None),
            _ => return Err(errno::INVAL),
        };
        list.push(Sub { userdata, tag, fd, deadline });
    }

    let mut ready: Vec<(u64, u8, i32, u16)> = Vec::new();
    let mut polled: Vec<usize> = Vec::new();
    let mut interest: Vec<PollFlags> = Vec::new();
    // Held until poll returns, so a concurrent close cannot hand a polled host fd to a new file.
    let mut held: Vec<Arc<Entry>> = Vec::new();
    for (i, s) in list.iter().enumerate() {
        if s.tag == EVENTTYPE_CLOCK {
            continue;
        }
        match k.get(s.fd) {
            Err(e) => ready.push((s.userdata, s.tag, e, 0)),
            Ok(entry) if pollable(&entry) => {
                interest.push(if s.tag == EVENTTYPE_FD_READ { PollFlags::IN } else { PollFlags::OUT });
                polled.push(i);
                held.push(entry);
            },
            Ok(_) => ready.push((s.userdata, s.tag, errno::SUCCESS, 0)),
        }
    }

    let wait = list.iter().filter_map(|s| s.deadline).min();
    let started = std::time::Instant::now();
    // `pollable` admitted only entries with a host fd.
    let mut pollfds: Vec<PollFd<'_>> = held.iter().zip(&interest).filter_map(|(e, ev)| Some(PollFd::from_borrowed_fd(e.fd().ok()?, *ev))).collect();
    if pollfds.is_empty() && ready.is_empty() {
        if let Some(w) = wait {
            std::thread::sleep(w);
        }
    } else {
        // Whole milliseconds, rounded up, as poll(2) takes them.
        let timeout = if !ready.is_empty() {
            Some(Timespec { tv_sec: 0, tv_nsec: 0 })
        } else {
            wait.map(|w| {
                let ms = w.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i64;
                Timespec { tv_sec: ms / 1000, tv_nsec: ((ms % 1000) * 1_000_000) as _ }
            })
        };
        loop {
            match rustix::event::poll(&mut pollfds, timeout.as_ref()) {
                Ok(_) => break,
                Err(rustix::io::Errno::INTR) => continue,
                Err(e) => return Err(errno::rx(e)),
            }
        }
        for (pfd, &i) in pollfds.iter().zip(&polled) {
            let revents = pfd.revents();
            if revents.is_empty() {
                continue;
            }
            let s = &list[i];
            let err = if revents.contains(PollFlags::NVAL) { errno::BADF } else { errno::SUCCESS };
            let flags = if revents.contains(PollFlags::HUP) { EVENT_HANGUP } else { 0 };
            ready.push((s.userdata, s.tag, err, flags));
        }
    }
    drop(pollfds);
    drop(held);

    if ready.is_empty() {
        let elapsed = started.elapsed();
        for s in &list {
            if let Some(d) = s.deadline {
                if d <= elapsed || Some(d) == wait {
                    ready.push((s.userdata, EVENTTYPE_CLOCK, errno::SUCCESS, 0));
                }
            }
        }
    }

    for (j, (userdata, tag, err, flags)) in ready.iter().enumerate() {
        let e = events + j as u32 * 32;
        m.set_u64(e, *userdata)?;
        m.set_u16(e + 8, *err as u16)?;
        m.set_u8(e + 10, *tag)?;
        m.set_u64(e + 16, 1)?;
        m.set_u16(e + 24, *flags)?;
    }
    if crate::trace() {
        let fds: Vec<(i32, u8)> = list.iter().filter(|s| s.tag != EVENTTYPE_CLOCK).map(|s| (s.fd, s.tag)).collect();
        eprintln!("[trace] poll_oneoff subs={fds:?} wait={wait:?} -> {:?}", ready.iter().map(|r| (r.1, r.2, r.3)).collect::<Vec<_>>());
    }
    m.set_u32(out, ready.len() as u32)
}

/// Whether to poll(2) the host fd of `e`; not when it is always ready: files, embedded files,
/// directories, and stdio that is a non-tty character device (macOS poll(2) reports POLLNVAL
/// for devices such as /dev/null).
fn pollable(e: &Entry) -> bool {
    match (&e.kind, e.fd()) {
        (Kind::Socket(_) | Kind::Pipe(_), Ok(_)) => true,
        (Kind::Stdio(_), Ok(fd)) => !matches!(Stat::of_fd(fd), Ok(st) if st.filetype == FILETYPE_CHAR && !fd.is_terminal()),
        _ => false,
    }
}
