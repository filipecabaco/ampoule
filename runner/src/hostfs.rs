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
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use rustix::fs::{AtFlags, FileType, Mode, OFlags};

use crate::errno;
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

fn open_dir_at(dir: impl AsFd, name: &CString) -> R<OwnedFd> {
    let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    rustix::fs::openat(dir, name.as_c_str(), flags, Mode::empty()).map_err(errno::rx)
}

/// Open a trusted host directory (a mount root chosen by the runtime or the user, not the guest).
fn open_root(path: &Path) -> R<OwnedFd> {
    let c = cstr(path.as_os_str().as_bytes())?;
    rustix::fs::open(c.as_c_str(), OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC, Mode::empty()).map_err(errno::rx)
}

/// Is `name` in `dir` a symlink (not following it)?
fn is_symlink_at(dir: impl AsFd, name: &CString) -> R<bool> {
    let st = rustix::fs::statat(dir, name.as_c_str(), AtFlags::SYMLINK_NOFOLLOW).map_err(errno::rx)?;
    Ok(FileType::from_raw_mode(st.st_mode as _) == FileType::Symlink)
}

pub fn readlink_at(dir: impl AsFd, name: &CString) -> R<Vec<u8>> {
    Ok(rustix::fs::readlinkat(dir, name.as_c_str(), Vec::new()).map_err(errno::rx)?.into_bytes())
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
                if let Ok(true) = is_symlink_at(&dir, &name) {
                    let target = readlink_at(&dir, &name)?;
                    return Ok(Walk::Redirect(redirect(&join(&parts[..i]), &target, &[])?));
                }
            }
            return Ok(Walk::Node(HostNode { dir, name, ro, mount_root: false, guest: join(&parts) }));
        }
        match open_dir_at(&dir, &name) {
            Ok(next) => dir = next,
            Err(e) => {
                if !is_symlink_at(&dir, &name).map_err(|_| e)? {
                    return Err(e);
                }
                let target = readlink_at(&dir, &name)?;
                return Ok(Walk::Redirect(redirect(&join(&parts[..i]), &target, &parts[i + 1..])?));
            },
        }
    }
    unreachable!("parts is never empty")
}

/// The entries of a host directory, opened safely: (name, inode, WASI filetype).
pub fn list(node: &HostNode) -> R<Vec<(Vec<u8>, u64, u8)>> {
    let dir = rustix::fs::Dir::new(open_dir_at(&node.dir, &node.name)?).map_err(errno::rx)?;
    let mut out = Vec::new();
    // A read error ends the listing, as readdir(3) returning NULL does.
    for ent in dir.map_while(Result::ok) {
        let name = ent.file_name().to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        out.push((name.to_vec(), ent.ino(), crate::wasi::filetype_of(ent.file_type())));
    }
    Ok(out)
}
