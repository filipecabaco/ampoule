//! State shared by every wasm thread: the fd table, the mount table, args, environment and the
//! network policy.
//!
//! The guest sees one tree rooted at "/" (its only preopen) built from mounts: the embedded
//! release at /app, host directories or files (/tmp, /etc/hosts, --mount), and virtual
//! directories joining them. Nothing outside a mount exists for the guest. Host-backed fds
//! (host files, sockets, pipes, stdio) go straight to the host kernel; embedded files are read
//! into memory when opened.

use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Instant, SystemTime};

use crate::errno;
use crate::hostfs::{self, HostNode, Walk};
use crate::mem::{Mem, R};
use crate::policy::Policy;
use crate::vfs::RootFs;

pub enum Kind {
    /// The runtime's own stdin/stdout/stderr, never closed by the guest.
    Stdio(RawFd),
    /// A host file; `ro` when it came from a read-only mount.
    File {
        fd: OwnedFd,
        ro: bool,
    },
    /// A file from the embedded rootfs, decompressed when opened.
    Mem {
        data: Arc<Vec<u8>>,
        pos: Mutex<u64>,
        ino: u64,
    },
    /// A directory, by guest path (resolved through the mounts on each use).
    Dir {
        guest: String,
    },
    Socket(OwnedFd),
    Pipe(OwnedFd),
}

pub struct Entry {
    pub kind: Kind,
}

impl Entry {
    pub fn raw(&self) -> Option<RawFd> {
        match &self.kind {
            Kind::Stdio(fd) => Some(*fd),
            Kind::File { fd, .. } | Kind::Socket(fd) | Kind::Pipe(fd) => Some(fd.as_raw_fd()),
            Kind::Mem { .. } | Kind::Dir { .. } => None,
        }
    }

    pub fn host_fd(&self) -> R<RawFd> {
        self.raw().ok_or(errno::BADF)
    }

    pub fn is_socket(&self) -> bool {
        matches!(self.kind, Kind::Socket(_))
    }
}

pub enum Backend {
    Embedded(Arc<RootFs>),
    /// The generated /proc (see `proc`).
    Proc,
    /// A host directory or file; `ro` refuses writes, creation and removal.
    Host {
        path: PathBuf,
        ro: bool,
    },
}

pub struct Mount {
    pub guest: String,
    pub backend: Backend,
}

/// Which mount a guest path falls in, before any symlink is resolved.
enum Target<'a> {
    Host {
        root: &'a Path,
        mount_guest: &'a str,
        /// The path below the mount, normalized and without a leading "/" ("" for the mount itself).
        rel: String,
        ro: bool,
    },
    Embedded {
        fs: &'a Arc<RootFs>,
        path: String,
    },
    Proc {
        path: String,
    },
    /// A directory that exists only because mounts live beneath it.
    Virtual,
    Missing,
}

/// What a guest path refers to, with every symlink on the way resolved inside the guest's
/// namespace (see `hostfs`).
pub enum Node<'a> {
    Host(HostNode),
    Embedded {
        fs: &'a Arc<RootFs>,
        path: String,
        guest: String,
    },
    Proc {
        path: String,
        guest: String,
    },
    /// A directory that exists only because mounts live beneath it.
    Virtual {
        guest: String,
    },
    Missing,
}

pub struct Kernel {
    pub mem: Mem,
    pub args: Vec<Vec<u8>>,
    pub env: Vec<Vec<u8>>,
    pub fds: RwLock<Vec<Option<Arc<Entry>>>>,
    pub mounts: Vec<Mount>,
    pub started: Instant,
    pub started_epoch: SystemTime,
    pub next_tid: AtomicI32,
    /// Running wasm threads besides the main one.
    pub live_threads: AtomicUsize,
    /// The write end of ERTS's signal-dispatcher pipe, once the guest registers it. Held by entry,
    /// not by fd number, so a later close or renumber in the guest cannot redirect signals.
    pub signal_pipe: OnceLock<Arc<Entry>>,
    pub policy: Policy,
}

/// Make `p` absolute and remove ".", ".." and empty components.
pub fn normalize(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {},
            ".." => {
                parts.pop();
            },
            c => parts.push(c),
        }
    }
    format!("/{}", parts.join("/"))
}

impl Kernel {
    pub fn new(mem: Mem, args: Vec<String>, env: Vec<(String, String)>, mut mounts: Vec<Mount>, policy: Policy) -> Kernel {
        let mut fds: Vec<Option<Arc<Entry>>> = (0..3).map(|fd| Some(Arc::new(Entry { kind: Kind::Stdio(fd) }))).collect();
        fds.push(Some(Arc::new(Entry { kind: Kind::Dir { guest: "/".into() } })));
        mounts.sort_by_key(|m| std::cmp::Reverse(m.guest.len()));
        Kernel {
            mem,
            args: args.into_iter().map(|a| a.into_bytes()).collect(),
            env: env.into_iter().map(|(k, v)| format!("{k}={v}").into_bytes()).collect(),
            fds: RwLock::new(fds),
            mounts,
            started: Instant::now(),
            started_epoch: SystemTime::now(),
            next_tid: AtomicI32::new(1),
            live_threads: AtomicUsize::new(0),
            signal_pipe: OnceLock::new(),
            policy,
        }
    }

    /// The guest's single preopen: "/" at fd 3.
    pub const PREOPEN_FD: i32 = 3;

    fn locate(&self, guest: &str) -> Target<'_> {
        for m in &self.mounts {
            let rest = if m.guest == "/" {
                Some(guest)
            } else if guest == m.guest {
                Some("/")
            } else {
                guest.strip_prefix(&m.guest).filter(|r| r.starts_with('/'))
            };
            if let Some(rest) = rest {
                return match &m.backend {
                    Backend::Embedded(fs) => Target::Embedded { fs, path: rest.to_string() },
                    Backend::Proc => Target::Proc { path: rest.to_string() },
                    Backend::Host { path, ro } => Target::Host { root: path, mount_guest: &m.guest, rel: rest.trim_start_matches('/').to_string(), ro: *ro },
                };
            }
        }
        let prefix = if guest == "/" { "/".to_string() } else { format!("{guest}/") };
        if self.mounts.iter().any(|m| m.guest.starts_with(&prefix)) {
            Target::Virtual
        } else {
            Target::Missing
        }
    }

    /// What `guest` refers to. `follow` also resolves a symlink in its last component.
    pub fn resolve(&self, guest: &str, follow: bool) -> R<Node<'_>> {
        let mut guest = normalize(guest);
        for _ in 0..=hostfs::MAX_HOPS {
            match self.locate(&guest) {
                Target::Host { root, mount_guest, rel, ro } => match hostfs::walk(root, mount_guest, &rel, ro, follow)? {
                    Walk::Node(node) => return Ok(Node::Host(node)),
                    Walk::Redirect(next) => guest = next,
                },
                Target::Embedded { fs, path } => return Ok(Node::Embedded { fs, path, guest }),
                Target::Proc { path } => return Ok(Node::Proc { path, guest }),
                Target::Virtual => return Ok(Node::Virtual { guest }),
                Target::Missing => return Ok(Node::Missing),
            }
        }
        Err(errno::LOOP)
    }

    /// Mount points directly below `guest`, which appear as directories in its listing.
    pub fn mount_children(&self, guest: &str) -> Vec<String> {
        let prefix = if guest == "/" { "/".to_string() } else { format!("{guest}/") };
        let mut names: Vec<String> = self
            .mounts
            .iter()
            .filter_map(|m| m.guest.strip_prefix(&prefix))
            .filter_map(|rest| rest.split('/').next())
            .filter(|n| !n.is_empty())
            .map(String::from)
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn get(&self, fd: i32) -> R<Arc<Entry>> {
        let fds = self.fds.read().unwrap();
        usize::try_from(fd).ok().and_then(|fd| fds.get(fd)).and_then(Clone::clone).ok_or(errno::BADF)
    }

    /// Install `kind` at the lowest free guest fd.
    pub fn insert(&self, kind: Kind) -> R<i32> {
        let mut fds = self.fds.write().unwrap();
        let entry = Some(Arc::new(Entry { kind }));
        if let Some(i) = fds.iter().skip(4).position(|e| e.is_none()) {
            fds[i + 4] = entry;
            return Ok((i + 4) as i32);
        }
        if fds.len() >= 4096 {
            return Err(errno::MFILE);
        }
        fds.push(entry);
        Ok((fds.len() - 1) as i32)
    }

    /// dup2-like: `from` takes the place of `to`, whose entry is closed.
    pub fn renumber(&self, from: i32, to: i32) -> R<()> {
        let (from, to) = (usize::try_from(from).map_err(|_| errno::BADF)?, usize::try_from(to).map_err(|_| errno::BADF)?);
        let replaced = {
            let mut fds = self.fds.write().unwrap();
            if fds.get(from).is_none_or(Option::is_none) || fds.get(to).is_none_or(Option::is_none) {
                return Err(errno::BADF);
            }
            if from == to {
                return Ok(());
            }
            let e = fds[from].take();
            std::mem::replace(&mut fds[to], e)
        };
        // Closed outside the lock: a close can block (SO_LINGER) and must not stall every fd call.
        drop(replaced);
        Ok(())
    }

    /// Take `fd` out of the table. The caller drops the entry outside the lock. A socket that
    /// another thread is still using (blocked in accept or recv, say) is shut down so that call
    /// returns; its host fd closes when the last user lets go.
    pub fn remove(&self, fd: i32) -> R<Arc<Entry>> {
        let e = {
            let mut fds = self.fds.write().unwrap();
            usize::try_from(fd).ok().and_then(|fd| fds.get_mut(fd)).and_then(Option::take).ok_or(errno::BADF)?
        };
        if let Kind::Socket(s) = &e.kind {
            if Arc::strong_count(&e) > 1 {
                unsafe { libc::shutdown(s.as_raw_fd(), libc::SHUT_RDWR) };
            }
        }
        Ok(e)
    }

    pub fn now_mono_ns(&self) -> u64 {
        self.started.elapsed().as_nanos() as u64
    }

    pub fn threads(&self) -> usize {
        self.live_threads.load(Ordering::Relaxed) + 1
    }

    /// A new thread id, or None once the range wasi-threads allows (1..=0x1FFFFFFF) is used up.
    pub fn tid(&self) -> Option<i32> {
        const MAX_TID: i32 = 0x1FFF_FFFF;
        let tid = self.next_tid.fetch_add(1, Ordering::SeqCst);
        (1..=MAX_TID).contains(&tid).then_some(tid)
    }
}

/// The host's /etc/hosts plus `extra` (name, ip) entries, written to `dir`.
pub fn hosts_file(dir: &std::path::Path, extra: &[(String, String)]) -> anyhow::Result<PathBuf> {
    let mut text = std::fs::read_to_string("/etc/hosts").unwrap_or_else(|_| "127.0.0.1\tlocalhost\n".into());
    if !text.ends_with('\n') {
        text.push('\n');
    }
    for (name, ip) in extra {
        text.push_str(&format!("{ip}\t{name}\n"));
    }
    let path = dir.join(".hosts");
    std::fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn kernel_with(mounts: Vec<Mount>) -> Kernel {
        let engine = wasmtime::Engine::new(&crate::engine::config()).unwrap();
        let memory = wasmtime::SharedMemory::new(&engine, wasmtime::MemoryType::shared(1, 1)).unwrap();
        Kernel::new(Mem::new(memory), Vec::new(), Vec::new(), mounts, Policy::default())
    }

    fn host(guest: &str, path: &str) -> Mount {
        Mount { guest: guest.into(), backend: Backend::Host { path: path.into(), ro: false } }
    }

    #[test]
    fn normalize_resolves_dots_and_stays_absolute() {
        assert_eq!(normalize("a/./b//../c"), "/a/c");
    }

    #[test]
    fn normalize_cannot_climb_above_root() {
        assert_eq!(normalize("/../../etc/passwd"), "/etc/passwd");
    }

    #[test]
    fn locate_maps_a_path_below_a_host_mount() {
        let k = kernel_with(vec![host("/tmp", "/host/tmp")]);
        assert!(matches!(k.locate("/tmp/x/y"), Target::Host { root, rel, .. } if root == Path::new("/host/tmp") && rel == "x/y"));
    }

    #[test]
    fn locate_prefers_the_longest_mount() {
        let k = kernel_with(vec![host("/etc", "/host/etc"), host("/etc/hosts", "/host/hosts")]);
        assert!(matches!(k.locate("/etc/hosts"), Target::Host { root, rel, .. } if root == Path::new("/host/hosts") && rel.is_empty()));
    }

    #[test]
    fn locate_does_not_match_a_sibling_with_a_common_prefix() {
        let k = kernel_with(vec![host("/tmp", "/host/tmp")]);
        assert!(matches!(k.locate("/tmpfoo"), Target::Missing));
    }

    #[test]
    fn locate_reports_parents_of_mounts_as_virtual() {
        let k = kernel_with(vec![host("/etc/hosts", "/host/hosts")]);
        assert!(matches!(k.locate("/etc"), Target::Virtual));
    }

    /// A kernel with a real host directory mounted at /tmp, and a second one at /data.
    fn sandbox() -> (crate::TestDir, Kernel) {
        let dir = crate::TestDir::new("sandbox");
        std::fs::create_dir_all(dir.0.join("tmp/sub")).unwrap();
        std::fs::create_dir_all(dir.0.join("data")).unwrap();
        std::fs::write(dir.0.join("tmp/sub/file"), b"inside").unwrap();
        std::fs::write(dir.0.join("secret"), b"outside every mount").unwrap();
        let tmp = dir.0.join("tmp").canonicalize().unwrap();
        let data = dir.0.join("data").canonicalize().unwrap();
        let k = kernel_with(vec![
            Mount { guest: "/tmp".into(), backend: Backend::Host { path: tmp, ro: false } },
            Mount { guest: "/data".into(), backend: Backend::Host { path: data, ro: true } },
        ]);
        (dir, k)
    }

    fn link(dir: &crate::TestDir, at: &str, target: &str) {
        std::os::unix::fs::symlink(target, dir.0.join(at)).unwrap();
    }

    #[test]
    fn resolve_finds_files_below_a_host_mount() {
        let (_dir, k) = sandbox();
        assert!(matches!(k.resolve("/tmp/sub/file", true), Ok(Node::Host(n)) if n.guest == "/tmp/sub/file" && !n.mount_root));
    }

    #[test]
    fn a_link_to_root_means_the_guests_root() {
        let (dir, k) = sandbox();
        link(&dir, "tmp/r", "/");
        assert!(matches!(k.resolve("/tmp/r", true), Ok(Node::Virtual { guest }) if guest == "/"));
    }

    #[test]
    fn an_absolute_link_cannot_reach_host_files() {
        let (dir, k) = sandbox();
        let secret = dir.0.join("secret").canonicalize().unwrap();
        link(&dir, "tmp/escape", secret.to_str().unwrap());
        // The target is read as a guest path: absent (Missing), or inside a guest mount that does
        // not have it (NOENT, when the host temp dir is itself under the guest's /tmp).
        assert!(matches!(k.resolve("/tmp/escape", true), Ok(Node::Missing) | Err(errno::NOENT)));
    }

    #[test]
    fn a_relative_link_cannot_climb_out_of_the_mount() {
        let (dir, k) = sandbox();
        link(&dir, "tmp/up", "../../../secret");
        assert!(matches!(k.resolve("/tmp/up", true), Ok(Node::Missing)));
    }

    #[test]
    fn a_link_through_a_directory_resolves_inside_the_mount() {
        let (dir, k) = sandbox();
        link(&dir, "tmp/d", "sub");
        assert!(matches!(k.resolve("/tmp/d/file", true), Ok(Node::Host(n)) if n.guest == "/tmp/sub/file"));
    }

    #[test]
    fn a_link_into_a_read_only_mount_stays_read_only() {
        let (dir, k) = sandbox();
        link(&dir, "tmp/ro", "/data");
        assert!(matches!(k.resolve("/tmp/ro/x", false), Ok(Node::Host(n)) if n.ro && n.guest == "/data/x"));
    }

    #[test]
    fn a_symlink_loop_is_eloop() {
        let (dir, k) = sandbox();
        link(&dir, "tmp/a", "b");
        link(&dir, "tmp/b", "a");
        assert_eq!(k.resolve("/tmp/a", true).err(), Some(errno::LOOP));
    }

    #[test]
    fn a_final_link_is_kept_unless_followed() {
        let (dir, k) = sandbox();
        link(&dir, "tmp/l", "/");
        assert!(matches!(k.resolve("/tmp/l", false), Ok(Node::Host(n)) if n.guest == "/tmp/l"));
    }

    #[test]
    fn a_mount_point_is_flagged() {
        let (_dir, k) = sandbox();
        assert!(matches!(k.resolve("/tmp", true), Ok(Node::Host(n)) if n.mount_root));
    }

    #[test]
    fn renumber_onto_itself_succeeds() {
        let k = kernel_with(Vec::new());
        let fd = k.insert(Kind::Dir { guest: "/".into() }).unwrap();
        assert_eq!(k.renumber(fd, fd), Ok(()));
    }

    #[test]
    fn renumber_moves_the_entry_and_frees_the_source() {
        let k = kernel_with(Vec::new());
        let a = k.insert(Kind::Dir { guest: "/a".into() }).unwrap();
        let b = k.insert(Kind::Dir { guest: "/b".into() }).unwrap();
        k.renumber(a, b).unwrap();
        assert!(matches!(&k.get(b).unwrap().kind, Kind::Dir { guest } if guest == "/a") && k.get(a).is_err());
    }

    #[test]
    fn thread_ids_stop_at_the_wasi_threads_limit() {
        let k = kernel_with(Vec::new());
        k.next_tid.store(0x1FFF_FFFF, Ordering::SeqCst);
        assert_eq!((k.tid(), k.tid()), (Some(0x1FFF_FFFF), None));
    }

    #[test]
    fn mount_children_lists_each_name_once() {
        let k = kernel_with(vec![host("/etc/hosts", "/a"), host("/etc/ssl/cert.pem", "/b"), host("/tmp", "/c")]);
        assert_eq!(k.mount_children("/etc"), vec!["hosts", "ssl"]);
    }

    #[test]
    fn insert_reuses_the_lowest_free_fd() {
        let k = kernel_with(Vec::new());
        let a = k.insert(Kind::Dir { guest: "/".into() }).unwrap();
        k.insert(Kind::Dir { guest: "/".into() }).unwrap();
        k.remove(a).unwrap();
        assert_eq!(k.insert(Kind::Dir { guest: "/".into() }).unwrap(), a);
    }
}
