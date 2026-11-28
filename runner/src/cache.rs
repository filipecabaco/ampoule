//! Relinked engines, kept between packs. Linking beam.wasm with a project's NIFs takes seconds of
//! LTO, and its output depends only on what the link reads, so an engine is stored under a hash
//! of exactly that: the kit, the wasi-sdk, this ampoule, and each NIF's archive and settings.

use std::path::{Path, PathBuf};

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::nif::BuiltNif;
use crate::toolchain::Toolchain;

/// Engines kept; older ones are removed when a new one is stored.
const KEEP: usize = 8;

/// `~/.cache/ampoule/engines`, or none with `AMPOULE_NO_CACHE` set (to anything but `0`).
pub fn dir() -> Option<PathBuf> {
    root().map(|r| r.join("engines"))
}

/// `~/.cache/ampoule/rust/<app>-<hash>`, where the Rust NIF at `krate` builds: one directory per
/// project and app, so two projects packed at once never share cargo's output. Emptied when this
/// ampoule, the wasi-sdk or the kit's ampoule-os headers change. None with `AMPOULE_NO_CACHE` set,
/// or when the directory cannot be made: that only costs the next pack a full build.
pub fn rust_dir(app: &str, krate: &Path, tc: &Toolchain) -> Option<PathBuf> {
    rust_dir_in(&root()?, app, krate, tc)
}

fn rust_dir_in(root: &Path, app: &str, krate: &Path, tc: &Toolchain) -> Option<PathBuf> {
    let mut h = Sha256::new();
    field(&mut h, krate.to_string_lossy().as_bytes());
    let dir = root.join("rust").join(format!("{app}-{}", &hex(h)[..16]));
    rust_key(tc).and_then(|key| keyed_dir(&dir, &key)).inspect_err(|e| eprintln!("==> not caching the Rust build of {app}: {e}")).ok()
}

fn rust_key(tc: &Toolchain) -> Result<String> {
    let mut h = Sha256::new();
    field(&mut h, b"ampoule-rust-1");
    field(&mut h, &std::fs::read(std::env::current_exe()?)?);
    field(&mut h, &std::fs::read(tc.wasi_sdk.join("VERSION")).unwrap_or_default());
    tree(&mut h, &tc.kit.join("ampoule-os"))?;
    Ok(hex(h))
}

fn keyed_dir(dir: &Path, key: &str) -> Result<PathBuf> {
    let stamp = dir.join("key");
    if std::fs::read_to_string(&stamp).ok().as_deref() != Some(key) {
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir)?;
        std::fs::write(&stamp, key)?;
    }
    Ok(dir.to_path_buf())
}

fn root() -> Option<PathBuf> {
    if std::env::var_os("AMPOULE_NO_CACHE").is_some_and(|v| v != "0" && !v.is_empty()) {
        return None;
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/ampoule"))
}

/// Hash of everything `nif::link_engine` reads.
pub fn key(tc: &Toolchain, nifs: &[BuiltNif]) -> Result<String> {
    let mut h = Sha256::new();
    field(&mut h, b"ampoule-engine-1");
    field(&mut h, env!("CARGO_PKG_VERSION").as_bytes());
    field(&mut h, &std::fs::read(tc.wasi_sdk.join("VERSION")).unwrap_or_default());
    tree(&mut h, &tc.kit)?;
    // The link order of NIFs is the order given, so it is part of the key.
    for n in nifs {
        field(&mut h, n.libname.as_bytes());
        field(&mut h, &[n.cxx as u8, n.rust as u8]);
        let mut libs = n.libs.clone();
        libs.sort();
        field(&mut h, libs.join(",").as_bytes());
        field(&mut h, &std::fs::read(&n.archive)?);
    }
    Ok(hex(h))
}

fn hex(h: Sha256) -> String {
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn tree(h: &mut Sha256, root: &Path) -> Result<()> {
    let mut files = Vec::new();
    walk(root, root, &mut files)?;
    files.sort();
    for rel in &files {
        field(h, rel.to_string_lossy().as_bytes());
        field(h, &std::fs::read(root.join(rel))?);
    }
    Ok(())
}

/// Length-prefixed, so no two different inputs hash the same bytes.
fn field(h: &mut Sha256, bytes: &[u8]) {
    h.update((bytes.len() as u64).to_le_bytes());
    h.update(bytes);
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            walk(root, &path, out)?;
        } else {
            out.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

/// The engine stored under `key`, marked as recently used.
pub fn get(dir: &Path, key: &str) -> Option<Vec<u8>> {
    let path = dir.join(format!("{key}.wasm"));
    let wasm = std::fs::read(&path).ok()?;
    // A wasm module starts with "\0asm"; anything else is a damaged entry, linked again.
    if !wasm.starts_with(b"\0asm") {
        let _ = std::fs::remove_file(&path);
        return None;
    }
    if let Ok(f) = std::fs::File::options().append(true).open(&path) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
    Some(wasm)
}

/// Store `wasm` under `key`: written to a temporary file and renamed, so a pack running at the
/// same time never reads half an engine. Failing to store only costs the next pack a link.
pub fn put(dir: &Path, key: &str, wasm: &[u8]) {
    let store = || -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".{key}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, wasm)?;
        std::fs::rename(&tmp, dir.join(format!("{key}.wasm"))).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    };
    if let Err(e) = store() {
        eprintln!("==> not caching the engine: {e}");
        return;
    }
    prune(dir, KEEP);
}

/// Keep the `keep` most recently used engines.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut engines: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "wasm"))
        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
        .collect();
    engines.sort_by_key(|e| std::cmp::Reverse(e.0));
    for (_, p) in engines.into_iter().skip(keep) {
        let _ = std::fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::{get, keyed_dir, prune, put, rust_dir_in, rust_key};
    use crate::toolchain::Toolchain;
    use std::path::Path;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("ampoule-cache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_stored_engine_comes_back_and_a_damaged_one_does_not() {
        let d = scratch("roundtrip");
        assert!(get(&d, "k").is_none());
        put(&d, "k", b"\0asm\x01\0\0\0");
        assert_eq!(get(&d, "k").as_deref(), Some(&b"\0asm\x01\0\0\0"[..]));
        std::fs::write(d.join("k.wasm"), b"garbage").unwrap();
        assert!(get(&d, "k").is_none());
        assert!(!d.join("k.wasm").exists());
        // No temporary files are left behind.
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn only_the_most_recently_used_engines_are_kept() {
        let d = scratch("prune");
        std::fs::create_dir_all(&d).unwrap();
        let now = std::time::SystemTime::now();
        for i in 0..5u64 {
            let p = d.join(format!("{i}.wasm"));
            std::fs::write(&p, b"\0asm").unwrap();
            let f = std::fs::File::options().append(true).open(&p).unwrap();
            f.set_modified(now - std::time::Duration::from_secs(100 - i)).unwrap();
        }
        std::fs::write(d.join("notes.txt"), b"").unwrap();
        prune(&d, 2);
        let mut left: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        left.sort();
        assert_eq!(left, ["3.wasm", "4.wasm", "notes.txt"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_rust_build_is_kept_under_the_same_key_and_emptied_under_another() {
        let d = scratch("keyed").join("textkit");
        keyed_dir(&d, "a").unwrap();
        std::fs::write(d.join("built"), b"").unwrap();
        keyed_dir(&d, "a").unwrap();
        assert!(d.join("built").exists());
        keyed_dir(&d, "b").unwrap();
        assert!(!d.join("built").exists());
        assert_eq!(std::fs::read_to_string(d.join("key")).unwrap(), "b");
        std::fs::remove_dir_all(d.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_rust_build_is_keyed_by_the_ampoule_os_headers_it_compiles_against() {
        let d = scratch("rust-key");
        let tc = Toolchain { wasi_sdk: d.join("wasi-sdk"), kit: d.join("kit") };
        std::fs::create_dir_all(tc.kit.join("ampoule-os")).unwrap();
        std::fs::write(tc.kit.join("ampoule-os/ampoule_os.h"), "int a;").unwrap();
        std::fs::write(tc.kit.join("liberts.a"), "1").unwrap();
        let key = rust_key(&tc).unwrap();

        std::fs::write(tc.kit.join("liberts.a"), "2").unwrap();
        assert_eq!(rust_key(&tc).unwrap(), key);
        std::fs::write(tc.kit.join("ampoule-os/ampoule_os.h"), "int b;").unwrap();
        assert_ne!(rust_key(&tc).unwrap(), key);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn each_project_builds_a_rust_nif_in_its_own_dir_or_without_the_cache() {
        let d = scratch("rust-dir");
        let tc = Toolchain { wasi_sdk: d.join("wasi-sdk"), kit: d.join("kit") };
        std::fs::create_dir_all(tc.kit.join("ampoule-os")).unwrap();
        let (a, b) = (Path::new("/a/_build/ampoule-nif/deps/mdex/native/x"), Path::new("/b/_build/ampoule-nif/deps/mdex/native/x"));
        let root = d.join("cache");
        let dir_a = rust_dir_in(&root, "mdex", a, &tc).unwrap();
        assert!(dir_a.is_dir() && dir_a.starts_with(root.join("rust")));
        assert_eq!(rust_dir_in(&root, "mdex", a, &tc).unwrap(), dir_a);
        assert_ne!(rust_dir_in(&root, "mdex", b, &tc).unwrap(), dir_a);

        // A cache that cannot be made (a read-only or missing HOME) builds without it.
        let file = d.join("not-a-dir");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(rust_dir_in(&file, "mdex", a, &tc), None);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
