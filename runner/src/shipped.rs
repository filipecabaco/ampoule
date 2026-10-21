//! Programs the release ships in its own priv/ directories (`/app/lib/<app>-<vsn>/priv/`). They are
//! the app's code, so ports start them without `[process] allow`, unless `[process] priv = false`;
//! otherwise they run as host programs do (see `spawn`).
//!
//! A packed release's files exist only in the embedded image, so the first start of a program from
//! an app's priv/ extracts that whole directory to the run's private /tmp, under
//! `.ampoule/priv/<app>-<vsn>/`, with the image's layout and modes: a program that finds files next
//! to it through its own path finds them there too. Each app's priv/ is extracted once per run. The
//! guest sees /tmp/.ampoule read-only (see `main::run_vm`), so it cannot change what runs. Only a
//! path that ends, after symlinks are resolved within the image, at a file below an app's priv/
//! qualifies. Run from a release directory (`ampoule run`), the files are on the host and run in
//! place.
//!
//! At pack time, `Program::detect` names the OS and CPU a priv/ executable was built for, so
//! packing can warn when it will not run on a target.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::errno;
use crate::kernel::{Kernel, Node};
use crate::mem::R;
use crate::vfs::{RootFs, KIND_DIR, KIND_FILE, KIND_SYMLINK};

/// `/lib/<app>-<vsn>/priv/<rel>`, a path in the release, -> (`<app>-<vsn>`, `<rel>`), when `<rel>`
/// is not empty and no component is "." or "..".
pub fn split(path: &str) -> Option<(&str, &str)> {
    let (dir, rest) = path.strip_prefix("/lib/")?.split_once('/')?;
    let rel = rest.strip_prefix("priv/")?;
    let plain = |c: &str| !c.is_empty() && c != "." && c != "..";
    (plain(dir) && rel.split('/').all(plain)).then_some((dir, rel))
}

/// The host file to execute for the guest path `guest` when it names a program the release ships
/// in a priv/ directory, extracting it first if need be. None when it does not (or such programs
/// may not run): the host-program policy decides then.
pub fn program(k: &Kernel, guest: &str) -> Option<R<PathBuf>> {
    let root = k.policy.exec.priv_dir.as_deref()?;
    match k.resolve(guest, true).ok()? {
        Node::Embedded { fs, path, .. } => {
            let (resolved, node) = fs.lookup(&path, true)?;
            let (app, rel) = split(&resolved)?;
            if node.kind != KIND_FILE {
                return None;
            }
            Some(extract(k, fs, root, app).and_then(|dir| executable(&dir.join(rel))))
        },
        // A release directory: the file is on the host already, if it is the release's own.
        Node::Host(n) if k.mount_point(&n.guest) == Some(crate::APP) => {
            split(n.guest.strip_prefix(crate::APP)?)?;
            Some(k.host_path(&n.guest))
        },
        _ => None,
    }
}

/// The app directory `app`'s priv/, extracted below `root` (once per run).
fn extract(k: &Kernel, fs: &RootFs, root: &Path, app: &str) -> R<PathBuf> {
    let mut done = k.extracted.lock().unwrap();
    if let Some(dir) = done.get(app) {
        return Ok(dir.clone());
    }
    let dir = root.join(app);
    // Left by an earlier run sharing this --tmp.
    let _ = std::fs::remove_dir_all(&dir);
    let from = format!("/lib/{app}/priv");
    if let Err(e) = write_tree(fs, &from, &from, &dir) {
        if crate::trace() {
            eprintln!("[trace] extracting {from} to {}: {e}", dir.display());
        }
        let _ = std::fs::remove_dir_all(&dir);
        return Err(errno::from_host(e.raw_os_error().unwrap_or(libc::EIO)));
    }
    done.insert(app.to_string(), dir.clone());
    Ok(dir)
}

/// `p`, made executable by its owner if the image did not have it so.
fn executable(p: &Path) -> R<PathBuf> {
    let io = |e: std::io::Error| errno::from_host(e.raw_os_error().unwrap_or(libc::EIO));
    let mode = std::fs::metadata(p).map_err(io)?.permissions().mode() & 0o777;
    if mode & 0o500 != 0o500 {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode | 0o500)).map_err(io)?;
    }
    Ok(p.to_path_buf())
}

/// Write the image directory `from` (inside `top`) and everything below it to `to`, which must not
/// exist. Each name is one component, so nothing lands outside `to`, and every file is created new,
/// never written through something already there. Symlinks are resolved within the image: one to
/// a file inside `top` becomes a copy of that file, any other is left out.
pub fn write_tree(fs: &RootFs, top: &str, from: &str, to: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new().mode(0o755).create(to)?;
    for (name, node) in fs.list(from) {
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            continue;
        }
        let (src, dst) = (format!("{from}/{name}"), to.join(&name));
        let (src, node) = match node.kind {
            KIND_SYMLINK => match fs.lookup(&src, true) {
                Some((p, n)) if n.kind == KIND_FILE && p.starts_with(&format!("{top}/")) => (p, n),
                _ => continue,
            },
            _ => (src, node),
        };
        match node.kind {
            KIND_DIR => write_tree(fs, top, &src, &dst)?,
            KIND_FILE => {
                // No setuid, setgid or sticky bits; the owner can always read it.
                let mode = node.mode & 0o777 | 0o400;
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&dst)?;
                f.write_all(&fs.read(node)?)?;
                // As the image has it, whatever the umask took away.
                f.set_permissions(std::fs::Permissions::from_mode(mode))?;
            },
            _ => {},
        }
    }
    Ok(())
}

/// The OS and CPUs an executable was built for, read from its ELF or Mach-O header.
#[derive(Debug, PartialEq)]
pub struct Program {
    /// "linux" (ELF) or "darwin" (Mach-O), as in target names.
    pub os: &'static str,
    /// As in target names: "x86_64", "aarch64", ...; several for a universal Mach-O.
    pub archs: Vec<&'static str>,
}

impl Program {
    /// The format of `data`, when it is an ELF or Mach-O (thin or universal) executable. Scripts
    /// (`#!`) and anything else are None.
    pub fn detect(data: &[u8]) -> Option<Program> {
        let be = |at: usize| Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?));
        let le = |at: usize| Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?));
        match data.get(0..4)? {
            b"\x7fELF" => {
                let at = data.get(18..20)?;
                let machine = match data.get(5)? {
                    2 => u16::from_be_bytes([at[0], at[1]]),
                    _ => u16::from_le_bytes([at[0], at[1]]),
                };
                Some(Program { os: "linux", archs: vec![elf_arch(machine)] })
            },
            [0xfe, 0xed, 0xfa, 0xce | 0xcf] => Some(Program { os: "darwin", archs: vec![macho_arch(be(4)?)] }),
            [0xce | 0xcf, 0xfa, 0xed, 0xfe] => Some(Program { os: "darwin", archs: vec![macho_arch(le(4)?)] }),
            // Universal: big-endian, a count, then one entry per architecture (20 bytes, or 32 in
            // the 64-bit form). Java class files share the magic; their version puts 45 or more
            // where the count is.
            [0xca, 0xfe, 0xba, b @ (0xbe | 0xbf)] => {
                let n = be(4)? as usize;
                let size = if *b == 0xbe { 20 } else { 32 };
                if n == 0 || n >= 20 {
                    return None;
                }
                let archs = (0..n).map(|i| be(8 + i * size).map(macho_arch)).collect::<Option<Vec<_>>>()?;
                Some(Program { os: "darwin", archs })
            },
            _ => None,
        }
    }

    /// Can it run on `target` (`<arch>-<os>`, as `--target` takes)?
    pub fn runs_on(&self, target: &str) -> bool {
        let (arch, os) = target.split_once('-').unwrap_or((target, ""));
        os == self.os && self.archs.contains(&arch)
    }

    /// "a macOS arm64 program", "a Linux x86_64 program".
    pub fn describe(&self) -> String {
        let darwin = self.os == "darwin";
        let archs: Vec<&str> = self.archs.iter().map(|a| if darwin && *a == "aarch64" { "arm64" } else { a }).collect();
        format!("a {} {} program", if darwin { "macOS" } else { "Linux" }, archs.join(" and "))
    }
}

fn elf_arch(machine: u16) -> &'static str {
    match machine {
        3 => "x86",
        0x14 => "powerpc",
        0x15 => "powerpc64",
        0x28 => "arm",
        0x3e => "x86_64",
        0xb7 => "aarch64",
        0xf3 => "riscv",
        _ => "unknown",
    }
}

fn macho_arch(cputype: u32) -> &'static str {
    match cputype {
        7 => "x86",
        0x0100_0007 => "x86_64",
        12 => "arm",
        0x0100_000c => "aarch64",
        0x0200_000c => "arm64_32",
        18 => "powerpc",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_takes_files_below_an_apps_priv() {
        assert_eq!(split("/lib/porter-0.1.0/priv/bin/greet"), Some(("porter-0.1.0", "bin/greet")));
        assert_eq!(split("/lib/my-app-1.0/priv/tool"), Some(("my-app-1.0", "tool")));
    }

    #[test]
    fn split_refuses_everything_else() {
        for p in [
            "/lib/porter-0.1.0/priv",
            "/lib/porter-0.1.0/priv/",
            "/lib/porter-0.1.0/ebin/porter.app",
            "/lib/porter-0.1.0/privx/tool",
            "/lib/porter-0.1.0/priv/../ebin/x",
            "/lib/porter-0.1.0/priv/bin/./tool",
            "/lib/../priv/tool",
            "/releases/0.1.0/priv/tool",
            "/erts-16.0/bin/beam",
            "lib/porter-0.1.0/priv/tool",
        ] {
            assert_eq!(split(p), None, "{p}");
        }
    }

    /// A release image with one app whose priv/ has a program, a helper beside it, a data file
    /// one level up, and links: one to a file inside priv/, one out of it, one to a directory.
    fn image() -> (crate::TestDir, std::sync::Arc<RootFs>) {
        let dir = crate::TestDir::new("shipped");
        let priv_dir = dir.0.join("rel/lib/app-1/priv");
        std::fs::create_dir_all(priv_dir.join("bin")).unwrap();
        std::fs::create_dir_all(dir.0.join("rel/lib/app-1/ebin")).unwrap();
        std::fs::write(priv_dir.join("bin/tool"), b"#!/bin/sh\necho tool\n").unwrap();
        std::fs::write(priv_dir.join("bin/helper.sh"), b"echo helper\n").unwrap();
        std::fs::write(priv_dir.join("data.txt"), b"data").unwrap();
        std::fs::write(dir.0.join("rel/lib/app-1/ebin/app.app"), b"{}").unwrap();
        std::fs::set_permissions(priv_dir.join("bin/tool"), std::fs::Permissions::from_mode(0o4755)).unwrap();
        std::os::unix::fs::symlink("../data.txt", priv_dir.join("bin/data")).unwrap();
        std::os::unix::fs::symlink("../../ebin/app.app", priv_dir.join("bin/out")).unwrap();
        std::os::unix::fs::symlink("/etc", priv_dir.join("etc")).unwrap();
        std::os::unix::fs::symlink("bin", priv_dir.join("bin2")).unwrap();
        let (image, _) = crate::pack::build_rootfs(&dir.0.join("rel")).unwrap();
        let fs = std::sync::Arc::new(RootFs::parse(Box::leak(image.into_boxed_slice())).unwrap());
        (dir, fs)
    }

    #[test]
    fn write_tree_keeps_the_layout_and_modes_inside_priv() {
        let (dir, fs) = image();
        let out = dir.0.join("out");
        write_tree(&fs, "/lib/app-1/priv", "/lib/app-1/priv", &out).unwrap();
        let mode = |p: &str| std::fs::metadata(out.join(p)).unwrap().permissions().mode() & 0o7777;
        assert_eq!(std::fs::read(out.join("bin/tool")).unwrap(), b"#!/bin/sh\necho tool\n");
        assert_eq!((mode("bin/tool"), std::fs::read(out.join("bin/helper.sh")).unwrap()), (0o755, b"echo helper\n".to_vec()));
        // A link to a file inside priv/ becomes a copy; links out of it, or to directories, are left out.
        assert_eq!(std::fs::read(out.join("bin/data")).unwrap(), b"data");
        for absent in ["bin/out", "etc", "bin2"] {
            assert!(std::fs::symlink_metadata(out.join(absent)).is_err(), "{absent}");
        }
    }

    /// A kernel with `fs` at /app, programs from priv/ extracted to `priv_dir`.
    fn kernel(fs: &std::sync::Arc<RootFs>, priv_dir: Option<PathBuf>) -> Kernel {
        let engine = wasmtime::Engine::new(&crate::engine::config()).unwrap();
        let memory = wasmtime::SharedMemory::new(&engine, wasmtime::MemoryType::shared(1, 1)).unwrap();
        let mounts = vec![crate::kernel::Mount { guest: crate::APP.into(), backend: crate::kernel::Backend::Embedded(fs.clone()) }];
        let mut policy = crate::policy::Policy::default();
        policy.exec.priv_dir = priv_dir;
        Kernel::new(crate::mem::Mem::new(memory), Vec::new(), Vec::new(), mounts, policy)
    }

    #[test]
    fn program_extracts_the_apps_priv_once_and_runs_from_there() {
        let (dir, fs) = image();
        let root = dir.0.join("priv");
        std::fs::create_dir(&root).unwrap();
        let k = kernel(&fs, Some(root.clone()));
        let tool = root.join("app-1/bin/tool");
        assert_eq!(program(&k, "/app/lib/app-1/priv/bin/tool"), Some(Ok(tool.clone())));
        assert!(root.join("app-1/bin/helper.sh").is_file() && root.join("app-1/data.txt").is_file());
        // Extracted once: a second start finds what the first left, through a link in the image too.
        std::fs::write(root.join("app-1/marker"), b"").unwrap();
        assert_eq!(program(&k, "/app/lib/app-1/priv/bin2/tool"), Some(Ok(tool)));
        assert!(root.join("app-1/marker").exists());
    }

    #[test]
    fn program_leaves_other_paths_to_the_policy() {
        let (dir, fs) = image();
        let k = kernel(&fs, Some(dir.0.clone()));
        for guest in ["/app/lib/app-1/ebin/app.app", "/app/lib/app-1/priv/bin/out", "/app/lib/app-1/priv/bin", "/app/lib/app-1/priv/nope", "/usr/bin/env"] {
            assert_eq!(program(&k, guest), None, "{guest}");
        }
        assert_eq!(program(&kernel(&fs, None), "/app/lib/app-1/priv/bin/tool"), None);
    }

    #[test]
    fn write_tree_never_writes_into_something_already_there() {
        let (dir, fs) = image();
        let out = dir.0.join("out");
        std::fs::create_dir_all(&out).unwrap();
        assert!(write_tree(&fs, "/lib/app-1/priv", "/lib/app-1/priv", &out).is_err());
    }

    #[test]
    fn detect_reads_elf_machines_in_either_byte_order() {
        let mut elf = b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x02\0\x3e\0".to_vec();
        assert_eq!(Program::detect(&elf), Some(Program { os: "linux", archs: vec!["x86_64"] }));
        elf[5] = 2;
        elf[18..20].copy_from_slice(&0xb7u16.to_be_bytes());
        assert_eq!(Program::detect(&elf), Some(Program { os: "linux", archs: vec!["aarch64"] }));
    }

    #[test]
    fn detect_reads_thin_and_universal_mach_o() {
        let thin = [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 0x01];
        assert_eq!(Program::detect(&thin), Some(Program { os: "darwin", archs: vec!["aarch64"] }));
        let mut fat = vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 2];
        for cpu in [0x0100_0007u32, 0x0100_000c] {
            fat.extend_from_slice(&cpu.to_be_bytes());
            fat.extend_from_slice(&[0; 16]);
        }
        let p = Program::detect(&fat).unwrap();
        assert_eq!(p.archs, vec!["x86_64", "aarch64"]);
        assert!(p.runs_on("aarch64-darwin") && p.runs_on("x86_64-darwin") && !p.runs_on("x86_64-linux"));
        assert_eq!(p.describe(), "a macOS x86_64 and arm64 program");
    }

    #[test]
    fn detect_ignores_scripts_java_classes_and_short_files() {
        let class = [0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 0x41];
        for data in [&b"#!/bin/sh\necho hi\n"[..], &class, b"\x7fEL", b""] {
            assert_eq!(Program::detect(data), None);
        }
    }

    #[test]
    fn runs_on_matches_os_and_arch() {
        let p = Program { os: "darwin", archs: vec!["aarch64"] };
        assert!(p.runs_on("aarch64-darwin") && !p.runs_on("aarch64-linux") && !p.runs_on("x86_64-darwin"));
        assert_eq!(p.describe(), "a macOS arm64 program");
    }
}
