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
    if std::env::var_os("AMPOULE_NO_CACHE").is_some_and(|v| v != "0" && !v.is_empty()) {
        return None;
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/ampoule/engines"))
}

/// Hash of everything `nif::link_engine` reads.
pub fn key(tc: &Toolchain, nifs: &[BuiltNif]) -> Result<String> {
    let mut h = Sha256::new();
    field(&mut h, b"ampoule-engine-1");
    field(&mut h, env!("CARGO_PKG_VERSION").as_bytes());
    field(&mut h, &std::fs::read(tc.wasi_sdk.join("VERSION")).unwrap_or_default());
    let mut files = Vec::new();
    walk(&tc.kit, &tc.kit, &mut files)?;
    files.sort();
    for rel in &files {
        field(&mut h, rel.to_string_lossy().as_bytes());
        field(&mut h, &std::fs::read(tc.kit.join(rel))?);
    }
    // The link order of NIFs is the order given, so it is part of the key.
    for n in nifs {
        field(&mut h, n.libname.as_bytes());
        field(&mut h, &[n.cxx as u8, n.rust as u8]);
        let mut libs = n.libs.clone();
        libs.sort();
        field(&mut h, libs.join(",").as_bytes());
        field(&mut h, &std::fs::read(&n.archive)?);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
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
    use super::{get, prune, put};

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
}
