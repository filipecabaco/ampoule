//! Host-backed paths, resolved without leaving the guest's namespace.
//!
//! A guest path inside a host mount is walked one component at a time from the mount's root
//! directory with `openat(O_NOFOLLOW)`. A symlink met on the way is read and resolved as a *guest*
//! path (an absolute target is relative to the guest's "/", a relative one to the link's guest
//! directory), so a link to "/" in /tmp means the guest's root, never the host's. The result is an
//! open parent directory plus the final name, and every operation on it is an `*at()` call that
//! does not follow a final symlink by itself. Holding a directory fd at each step also means a
//! component swapped for a symlink mid-walk is seen as a link, not followed.

use std::ffi::CString;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::errno::{self, check};
use crate::mem::R;

/// How many symlinks one lookup may pass through (Linux's MAXSYMLINKS).
pub const MAX_HOPS: usize = 40;

/// A host object named by a guest path: its parent directory and its name in it.
pub struct HostNode {
    pub dir: OwnedFd,
    pub name: CString,
    /// The mount refuses writes, creation and removal.
    pub ro: bool,
    /// The node is a mount point itself: it can be used but not removed, renamed or replaced.
    pub mount_root: bool,
    /// The guest path after resolving symlinks.
    pub guest: String,
}

/// One step of a walk: done, or a symlink to resolve from a new guest path.
pub enum Walk {
    Node(HostNode),
    Redirect(String),
}

fn cstr(s: &[u8]) -> R<CString> {
    CString::new(s).map_err(|_| errno::INVAL)
}

fn open_dir_at(dir: RawFd, name: &CString) -> R<OwnedFd> {
    let fd = check(unsafe { libc::openat(dir, name.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) })?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Open a trusted host directory (a mount root chosen by the runtime or the user, not the guest).
fn open_root(path: &Path) -> R<OwnedFd> {
    let c = cstr(path.as_os_str().as_bytes())?;
    let fd = check(unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) })?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// `lstat` of `name` in `dir`.
pub fn lstat_at(dir: RawFd, name: &CString) -> R<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    check(unsafe { libc::fstatat(dir, name.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) })?;
    Ok(st)
}

pub fn is_symlink(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFLNK
}

pub fn readlink_at(dir: RawFd, name: &CString) -> R<Vec<u8>> {
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    let n = errno::check_len(unsafe { libc::readlinkat(dir, name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) })?;
    buf.truncate(n);
    Ok(buf)
}

/// Where a symlink found at guest directory `dir_guest` with target `target` points, plus the
/// components still to walk after it.
fn redirect(dir_guest: &str, target: &[u8], rest: &[&str]) -> R<String> {
    let target = std::str::from_utf8(target).map_err(|_| errno::INVAL)?;
    let base = if target.starts_with('/') { target.to_string() } else { format!("{dir_guest}/{target}") };
    let mut path = base;
    for c in rest {
        path.push('/');
        path.push_str(c);
    }
    Ok(crate::kernel::normalize(&path))
}

/// Walk `rel` (a normalized path relative to the mount, "" for the mount itself) beneath the host
/// directory or file `root`, mounted at `mount_guest`. `follow` resolves a symlink in the last
/// component too.
pub fn walk(root: &Path, mount_guest: &str, rel: &str, ro: bool, follow: bool) -> R<Walk> {
    let join = |parts: &[&str]| -> String {
        let mut g = mount_guest.trim_end_matches('/').to_string();
        for p in parts {
            g.push('/');
            g.push_str(p);
        }
        if g.is_empty() {
            "/".into()
        } else {
            g
        }
    };
    if rel.is_empty() {
        // The mount point itself: a directory is used as "." in it; a file through its parent.
        let node = if root.is_dir() {
            HostNode { dir: open_root(root)?, name: cstr(b".")?, ro, mount_root: true, guest: join(&[]) }
        } else {
            let parent = root.parent().ok_or(errno::NOENT)?;
            let name = root.file_name().ok_or(errno::NOENT)?;
            HostNode { dir: open_root(parent)?, name: cstr(name.as_bytes())?, ro, mount_root: true, guest: join(&[]) }
        };
        return Ok(Walk::Node(node));
    }
    if !root.is_dir() {
        return Err(errno::NOTDIR);
    }
    let parts: Vec<&str> = rel.split('/').collect();
    let mut dir = open_root(root)?;
    for (i, part) in parts.iter().enumerate() {
        let name = cstr(part.as_bytes())?;
        let last = i + 1 == parts.len();
        if last {
            if follow {
                if let Ok(st) = lstat_at(dir.as_raw_fd(), &name) {
                    if is_symlink(&st) {
                        let target = readlink_at(dir.as_raw_fd(), &name)?;
                        return Ok(Walk::Redirect(redirect(&join(&parts[..i]), &target, &[])?));
                    }
                }
            }
            return Ok(Walk::Node(HostNode { dir, name, ro, mount_root: false, guest: join(&parts) }));
        }
        match open_dir_at(dir.as_raw_fd(), &name) {
            Ok(next) => dir = next,
            Err(e) => {
                let st = lstat_at(dir.as_raw_fd(), &name).map_err(|_| e)?;
                if !is_symlink(&st) {
                    return Err(e);
                }
                let target = readlink_at(dir.as_raw_fd(), &name)?;
                return Ok(Walk::Redirect(redirect(&join(&parts[..i]), &target, &parts[i + 1..])?));
            },
        }
    }
    unreachable!("parts is never empty")
}

/// The entries of a host directory, opened safely: (name, inode, WASI filetype).
pub fn list(node: &HostNode) -> R<Vec<(Vec<u8>, u64, u8)>> {
    let raw = open_dir_at(node.dir.as_raw_fd(), &node.name)?.into_raw_fd();
    // fdopendir takes ownership of the fd: closedir closes it.
    let dirp = unsafe { libc::fdopendir(raw) };
    if dirp.is_null() {
        let e = errno::last();
        unsafe { libc::close(raw) };
        return Err(e);
    }
    let mut out = Vec::new();
    loop {
        let ent = unsafe { libc::readdir(dirp) };
        if ent.is_null() {
            break;
        }
        // SAFETY: readdir returned a valid entry, used before the next readdir/closedir call.
        let ent = unsafe { &*ent };
        let name = unsafe { std::ffi::CStr::from_ptr(ent.d_name.as_ptr()) }.to_bytes().to_vec();
        if name == b"." || name == b".." {
            continue;
        }
        let ft = match ent.d_type {
            libc::DT_DIR => crate::wasi::FILETYPE_DIR,
            libc::DT_REG => crate::wasi::FILETYPE_REGULAR,
            libc::DT_LNK => crate::wasi::FILETYPE_SYMLINK,
            libc::DT_SOCK => crate::wasi::FILETYPE_SOCKET_STREAM,
            libc::DT_CHR => crate::wasi::FILETYPE_CHAR,
            libc::DT_BLK => crate::wasi::FILETYPE_BLOCK,
            _ => crate::wasi::FILETYPE_UNKNOWN,
        };
        out.push((name, ent.d_ino, ft));
    }
    unsafe { libc::closedir(dirp) };
    Ok(out)
}
