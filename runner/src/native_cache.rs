//! The engine, decompressed once and mapped from a file.
//!
//! A packed binary carries its precompiled engine (ERTS for wasm, compiled for this host) zstd
//! compressed. Decompressing it at every start and handing the bytes to `Module::deserialize`
//! copies the engine's code and data into anonymous memory: about 16 MiB that stays private and
//! dirty for the VM's whole life, a third of a small app's footprint, which the kernel can never
//! reclaim. Written once to a file and loaded with `Module::deserialize_open_file` instead, the
//! engine is mapped from that file: clean, shared pages the kernel drops under pressure and reads
//! back when needed, and later starts skip the decompression.
//!
//! The file is code this process runs. The binary carries the SHA-256 of the engine's native code
//! next to the compressed code (`engine_target::precompile`), and a file is only mapped when its
//! bytes hash to it: a torn write, a damaged disk or a file someone put there is never run, and is
//! written again. It is also only used from a directory and file that belong to this user and
//! that nobody else can write to. Anything else falls back to decompressing in memory, as does
//! every failure here, and says why.
//!
//! The directory must never be mounted read-write into an app (`--mount`): the app could change a
//! file after it was checked, and the pages read from it later would run the change. A VM whose
//! own mounts reach the directory does not use it.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::hash::BuildHasher;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};
use wasmtime::{Engine, Module};

/// Engines kept; older ones are removed when a new one is written.
const KEEP: usize = 4;

/// A temporary file older than this was left by a write that never finished.
const STALE_TMP: Duration = Duration::from_secs(3600);

/// The SHA-256 of the native code and the compressed code, from an engine as a binary carries it.
pub fn split(packed: &[u8]) -> Option<(&[u8; 32], &[u8])> {
    packed.split_first_chunk::<32>()
}

/// Whether and where to keep the engine: `native_cache` in ampoule.toml, packed into the binary, or
/// `--native-cache`/`AMPOULE_NATIVE_CACHE` at run time, which win over it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Setting {
    /// A cache directory if one is writable (below): the default.
    #[default]
    Auto,
    /// Never: the engine is loaded in memory at every start, and nothing is written. For a board
    /// whose storage should not be written to.
    Off,
    /// This directory: one several instances share, so the kernel keeps one copy of the engine.
    Dir(PathBuf),
}

impl Setting {
    /// "auto", "off", or an absolute directory.
    pub fn parse(v: &str) -> anyhow::Result<Setting> {
        match v {
            "auto" => Ok(Setting::Auto),
            "off" => Ok(Setting::Off),
            dir if Path::new(dir).is_absolute() => Ok(Setting::Dir(PathBuf::from(dir))),
            other => anyhow::bail!("native cache is auto, off or an absolute directory, not {other:?}"),
        }
    }
}

/// Where engines are kept: the setting's directory, or for `Auto`, `AMPOULE_NATIVE_CACHE`, systemd's
/// `CACHE_DIRECTORY` (a service with `CacheDirectory=`), `$XDG_CACHE_HOME/ampoule/native` or
/// `~/.cache/ampoule/native`. None for `Off`, with `AMPOULE_NO_CACHE` set (to anything but `0`), or
/// when one of `writable`, the host paths the app can write, contains it.
pub fn dir(setting: &Setting, writable: &[&Path]) -> Option<PathBuf> {
    let env = |name: &str| std::env::var_os(name);
    let dir = match setting {
        Setting::Off => return None,
        _ if env("AMPOULE_NO_CACHE").is_some_and(|v| !v.is_empty() && v != "0") => return None,
        Setting::Dir(d) => d.clone(),
        Setting::Auto => dir_from(env)?,
    };
    if let Some(w) = exposed(&dir, writable) {
        eprintln!("ampoule: not using the engine cache in {}: the app can write to it through {}; loading the engine in memory", dir.display(), w.display());
        return None;
    }
    Some(dir)
}

fn dir_from(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| var(name).filter(|v| !v.is_empty());
    if set("AMPOULE_NO_CACHE").is_some_and(|v| v != "0") {
        return None;
    }
    if let Some(d) = set("AMPOULE_NATIVE_CACHE") {
        return Some(PathBuf::from(d));
    }
    // systemd lists one or more directories, separated by colons; the first is the unit's own.
    if let Some(d) = set("CACHE_DIRECTORY").and_then(|v| v.to_str().and_then(|s| s.split(':').next()).map(PathBuf::from)) {
        return Some(d.join("ampoule"));
    }
    // A relative XDG_CACHE_HOME is invalid, and the spec says to ignore it.
    if let Some(d) = set("XDG_CACHE_HOME").map(PathBuf::from).filter(|d| d.is_absolute()) {
        return Some(d.join("ampoule/native"));
    }
    set("HOME").map(|h| PathBuf::from(h).join(".cache/ampoule/native"))
}

/// The first of `writable` that contains `dir`, symlinks resolved.
fn exposed<'a>(dir: &Path, writable: &[&'a Path]) -> Option<&'a Path> {
    let dir = resolved(dir);
    writable.iter().copied().find(|w| dir.starts_with(resolved(w)))
}

/// `p` absolute, with symlinks resolved as far as it exists: the cache directory may not yet.
fn resolved(p: &Path) -> PathBuf {
    let p = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut missing = Vec::new();
    let mut at = p.as_path();
    loop {
        if let Ok(real) = at.canonicalize() {
            return missing.iter().rev().fold(real, |path, name| path.join(name));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_owned());
                at = parent;
            },
            _ => return p,
        }
    }
}

/// The engine whose native code hashes to `digest`, mapped from a file in `dir`, written there
/// first from `compressed` when it is missing or damaged. On `Err` the caller loads the engine in
/// memory, from the decompressed code when this already made it: never decompressed twice.
pub fn load(engine: &Engine, digest: &[u8; 32], compressed: &[u8], dir: &Path) -> Result<Module, Option<Vec<u8>>> {
    let key = hex(digest);
    let path = dir.join(format!("{key}.cwasm"));
    // Left when a checked engine could not be mapped (a noexec mount, say), so that later starts
    // don't write it and fail again every time.
    let unusable = dir.join(format!("{key}.unusable"));

    if let Err(why) = ready(dir) {
        return Err(skip(dir, &why, None));
    }
    if unusable.exists() {
        return Err(skip(dir, &format!("mapping the engine from it failed before; remove {} to try again", unusable.display()), None));
    }
    match open_checked(&path, digest) {
        Ok(Some(file)) => {
            // Marks it used, so `prune` keeps it.
            let _ = file.set_modified(SystemTime::now());
            return map(engine, file, &path, &unusable, None);
        },
        Ok(None) => {},
        Err(why) => return Err(skip(dir, &why, None)),
    }

    let Ok(native) = zstd::bulk::decompress(compressed, 512 << 20) else { return Err(None) };
    // Only what the digest names is written; a damaged binary is for the in-memory load to report.
    if Sha256::digest(&native)[..] != digest[..] {
        return Err(Some(native));
    }
    let file = match store(dir, &path, &native) {
        Ok(file) => file,
        Err(e) => return Err(skip(dir, &format!("cannot write to it ({e})"), Some(native))),
    };
    prune(dir, KEEP);
    map(engine, file, &path, &unusable, Some(native))
}

/// The engine mapped from `file`, checked by `open_checked` or just written by `store`. When that
/// fails, the file is replaced by the `unusable` marker.
fn map(engine: &Engine, file: File, path: &Path, unusable: &Path, native: Option<Vec<u8>>) -> Result<Module, Option<Vec<u8>>> {
    // SAFETY: `file` is a regular file of this user's that group and others cannot write, in a
    // directory of the same kind, and when it was opened its bytes were exactly the native code
    // this binary carries (they hash to the digest built into the binary with the code), produced
    // by wasmtime for this engine's configuration; wasmtime also checks its version and
    // configuration header. Nothing keeps it so while mapped: pages are read from the file as they
    // are used, so a process of this user's that made the file writable and changed it would change
    // the code run. Only this user's own processes can, which is why a VM whose mounts let its app
    // write the directory does not use it (`dir`), and why it must never be mounted read-write
    // into an app.
    match unsafe { Module::deserialize_open_file(engine, file) } {
        Ok(module) => Ok(module),
        Err(e) => {
            let _ = std::fs::remove_file(path);
            let _ = File::create(unusable);
            let dir = path.parent().unwrap_or(path);
            Err(skip(dir, &format!("mapping the engine from it failed ({e:#}); remove {} to try again", unusable.display()), native))
        },
    }
}

/// Says why the cache is not used, and hands back what the caller loads instead.
fn skip(dir: &Path, why: &str, native: Option<Vec<u8>>) -> Option<Vec<u8>> {
    eprintln!("ampoule: not using the engine cache in {}: {why}; loading the engine in memory (AMPOULE_NO_CACHE=1 skips the cache)", dir.display());
    native
}

/// Makes `dir`, private, when it is missing; says why when it cannot hold engines.
fn ready(dir: &Path) -> Result<(), String> {
    if std::fs::symlink_metadata(dir).is_err() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| format!("cannot create it ({e})"))?;
        // The umask may have taken the owner's own bits. Only a directory made here is changed:
        // one the user named keeps its mode, and is checked below like any other.
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    if !std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir() && private(&m)) {
        return Err("it is not a directory of this user's that only they can write".into());
    }
    Ok(())
}

/// The engine at `path`, open, when it can be trusted: a regular file of this user's that nobody
/// else can write, whose bytes hash to `digest`. `Ok(None)` when it is missing, or was damaged and
/// has been removed; `Err` when it is not this user's alone.
fn open_checked(path: &Path, digest: &[u8; 32]) -> Result<Option<File>, String> {
    let mut file = match OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot open {} ({e})", path.display())),
    };
    // Checked on the open file, the one hashed and then mapped: replacing the path afterwards
    // changes nothing here.
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || !private(&meta) {
        return Err(format!("{} is not a file of this user's that only they can write", path.display()));
    }
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {},
            Err(e) => return Err(format!("cannot read {} ({e})", path.display())),
        }
    }
    if h.finalize()[..] != digest[..] {
        // A torn write or a damaged disk: written again.
        let _ = std::fs::remove_file(path);
        return Ok(None);
    }
    Ok(Some(file))
}

/// Owned by this user, and neither group nor others can write it.
fn private(m: &std::fs::Metadata) -> bool {
    m.uid() == rustix::process::geteuid().as_raw() && m.mode() & 0o022 == 0
}

/// Written to a new file aside, synced, and renamed into place, read-only, so a VM starting at the
/// same time never maps half an engine and a crash never leaves one under the engine's name. The
/// file stays open: it is the one mapped, with nothing to hash again.
fn store(dir: &Path, path: &Path, native: &[u8]) -> std::io::Result<File> {
    let tmp = dir.join(tmp_name());
    // create_new: never a file left there, nor one a symlink points to.
    let mut file = OpenOptions::new().read(true).write(true).create_new(true).mode(0o400).open(&tmp)?;
    let written = file.write_all(native).and_then(|()| file.sync_all()).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // The rename lasts through a crash once the directory is synced too.
    let _ = File::open(dir).and_then(|d| d.sync_all());
    Ok(file)
}

/// A name no other writer uses, even VMs in containers that are all pid 1 sharing the directory.
fn tmp_name() -> String {
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    // RandomState is seeded from the OS's random source.
    let random = std::collections::hash_map::RandomState::new().hash_one(nanos);
    format!(".{}-{nanos:x}-{random:016x}.tmp", std::process::id())
}

/// Keeps the `keep` most recently used engines (and markers), and removes temporary files a write
/// that never finished left behind.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let now = SystemTime::now();
    let (mut engines, mut markers) = (Vec::new(), Vec::new());
    for path in entries.filter_map(|e| e.ok()).map(|e| e.path()) {
        let Some(modified) = path.symlink_metadata().ok().and_then(|m| m.modified().ok()) else { continue };
        match path.extension().and_then(|x| x.to_str()) {
            Some("cwasm") => engines.push((modified, path)),
            Some("unusable") => markers.push((modified, path)),
            Some("tmp") if now.duration_since(modified).is_ok_and(|age| age > STALE_TMP) => {
                let _ = std::fs::remove_file(path);
            },
            _ => {},
        }
    }
    for mut found in [engines, markers] {
        found.sort_by_key(|e| std::cmp::Reverse(e.0));
        for (_, p) in found.into_iter().skip(keep) {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestDir;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn set_mode(p: &Path, mode: u32) {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    fn age(p: &Path, secs: u64) {
        File::open(p).unwrap().set_modified(SystemTime::now() - Duration::from_secs(secs)).unwrap();
    }

    fn digest(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }

    #[test]
    fn the_setting_is_auto_off_or_an_absolute_directory() {
        assert_eq!(Setting::parse("auto").unwrap(), Setting::Auto);
        assert_eq!(Setting::parse("off").unwrap(), Setting::Off);
        assert_eq!(Setting::parse("/var/cache/x").unwrap(), Setting::Dir(PathBuf::from("/var/cache/x")));
        assert!(Setting::parse("cache").is_err() && Setting::parse("").is_err() && Setting::parse("Off").is_err());
        assert_eq!(dir(&Setting::Off, &[]), None);
        assert_eq!(dir(&Setting::Dir(PathBuf::from("/shared/engines")), &[]), Some(PathBuf::from("/shared/engines")));
        // A directory the app can write is never used, whoever chose it.
        assert_eq!(dir(&Setting::Dir(PathBuf::from("/data/engines")), &[Path::new("/data")]), None);
    }

    #[test]
    fn the_directory_comes_from_the_first_variable_set() {
        let from = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, String)> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            dir_from(|name| vars.iter().find(|(k, _)| k == name).map(|(_, v)| OsString::from(v)))
        };
        let all = [("AMPOULE_NATIVE_CACHE", "/a"), ("CACHE_DIRECTORY", "/b:/c"), ("XDG_CACHE_HOME", "/d"), ("HOME", "/e")];
        assert_eq!(from(&all), Some(PathBuf::from("/a")));
        assert_eq!(from(&all[1..]), Some(PathBuf::from("/b/ampoule")));
        assert_eq!(from(&all[2..]), Some(PathBuf::from("/d/ampoule/native")));
        assert_eq!(from(&all[3..]), Some(PathBuf::from("/e/.cache/ampoule/native")));
        assert_eq!(from(&[("XDG_CACHE_HOME", "rel"), ("HOME", "/e")]), Some(PathBuf::from("/e/.cache/ampoule/native")));
        assert_eq!(from(&[("AMPOULE_NATIVE_CACHE", ""), ("HOME", "/e")]), Some(PathBuf::from("/e/.cache/ampoule/native")));
        assert_eq!(from(&[("AMPOULE_NO_CACHE", "1"), ("AMPOULE_NATIVE_CACHE", "/a")]), None);
        assert_eq!(from(&[("AMPOULE_NO_CACHE", "0"), ("AMPOULE_NATIVE_CACHE", "/a")]), Some(PathBuf::from("/a")));
        assert_eq!(from(&[]), None);
    }

    #[test]
    fn a_directory_the_app_can_write_is_exposed() {
        let t = TestDir::new("native-exposed");
        let shared = t.0.join("shared");
        std::fs::create_dir(&shared).unwrap();
        let cache = shared.join("not-yet/native");
        assert_eq!(exposed(&cache, &[Path::new("/nonexistent"), &shared]), Some(shared.as_path()));
        // Through a symlink to it as well.
        std::os::unix::fs::symlink(&shared, t.0.join("link")).unwrap();
        assert!(exposed(&t.0.join("link/native"), &[&shared]).is_some());
        assert!(exposed(&cache, &[&t.0.join("other")]).is_none());
    }

    #[test]
    fn a_stored_engine_is_read_only_and_checked_against_its_digest() {
        let t = TestDir::new("native-store");
        let path = t.0.join("k.cwasm");
        drop(store(&t.0, &path, b"engine").unwrap());
        assert_eq!((std::fs::read(&path).unwrap(), mode(&path)), (b"engine".to_vec(), 0o400));
        assert!(open_checked(&path, &digest(b"engine")).unwrap().is_some());
        // Other bytes under the engine's name: removed, to be written again.
        assert!(open_checked(&path, &digest(b"other")).unwrap().is_none() && !path.exists());
        assert!(open_checked(&path, &digest(b"engine")).unwrap().is_none());
    }

    #[test]
    fn a_file_or_directory_others_can_write_is_not_trusted() {
        let t = TestDir::new("native-writable");
        let path = t.0.join("k.cwasm");
        drop(store(&t.0, &path, b"engine").unwrap());
        set_mode(&path, 0o666);
        assert!(open_checked(&path, &digest(b"engine")).is_err());
        set_mode(&path, 0o400);
        std::os::unix::fs::symlink(&path, t.0.join("link.cwasm")).unwrap();
        assert!(open_checked(&t.0.join("link.cwasm"), &digest(b"engine")).is_err());
        set_mode(&t.0, 0o777);
        assert!(ready(&t.0).is_err());
        // A directory the user made keeps its mode when nobody else can write it.
        set_mode(&t.0, 0o755);
        assert!(ready(&t.0).is_ok() && mode(&t.0) == 0o755);
    }

    #[test]
    fn a_missing_directory_is_made_private() {
        let t = TestDir::new("native-missing");
        let dir = t.0.join("a/b");
        ready(&dir).unwrap();
        assert_eq!(mode(&dir), 0o700);
        // One that cannot be made, as on a read-only file system.
        std::fs::write(t.0.join("file"), b"").unwrap();
        assert!(ready(&t.0.join("file/native")).is_err());
    }

    #[test]
    fn temporary_names_do_not_repeat() {
        let (a, b) = (tmp_name(), tmp_name());
        assert!(a != b && a.starts_with(&format!(".{}-", std::process::id())) && a.ends_with(".tmp"));
    }

    #[test]
    fn only_the_most_recently_used_engines_and_fresh_temporary_files_are_kept() {
        let t = TestDir::new("native-prune");
        for (i, name) in ["a", "b", "c"].iter().enumerate() {
            let path = t.0.join(format!("{name}.cwasm"));
            drop(store(&t.0, &path, b"x").unwrap());
            age(&path, 100 - i as u64);
        }
        for name in [".old.tmp", ".new.tmp"] {
            std::fs::write(t.0.join(name), b"").unwrap();
        }
        age(&t.0.join(".old.tmp"), 2 * 3600);
        prune(&t.0, 2);
        assert!(!t.0.join("a.cwasm").exists() && t.0.join("b.cwasm").exists() && t.0.join("c.cwasm").exists());
        assert!(!t.0.join(".old.tmp").exists() && t.0.join(".new.tmp").exists());
    }

    /// The engine of a module with nothing in it: real native code, for the load paths.
    #[cfg(feature = "compiler")]
    fn tiny() -> (Engine, Vec<u8>, [u8; 32], Vec<u8>) {
        let engine = Engine::new(&crate::engine::config()).unwrap();
        let native = engine.precompile_module(b"\0asm\x01\0\0\0").unwrap();
        let compressed = zstd::bulk::compress(&native, 3).unwrap();
        (engine, native.clone(), digest(&native), compressed)
    }

    #[cfg(feature = "compiler")]
    #[test]
    fn an_engine_is_written_once_then_mapped() {
        let (engine, native, digest, compressed) = tiny();
        let t = TestDir::new("native-load");
        let path = t.0.join(format!("{}.cwasm", hex(&digest)));
        assert!(load(&engine, &digest, &compressed, &t.0).is_ok());
        assert_eq!((std::fs::read(&path).unwrap(), mode(&path)), (native.clone(), 0o400));
        // A hit does not decompress: what it would decompress can be anything.
        assert!(load(&engine, &digest, b"not zstd", &t.0).is_ok());

        // Damaged: written again from the binary's copy.
        set_mode(&path, 0o600);
        std::fs::write(&path, b"garbage").unwrap();
        set_mode(&path, 0o400);
        assert!(load(&engine, &digest, &compressed, &t.0).is_ok());
        assert_eq!(std::fs::read(&path).unwrap(), native);
    }

    #[cfg(feature = "compiler")]
    #[test]
    fn an_unusable_cache_falls_back_without_decompressing_twice() {
        let (engine, native, digest, compressed) = tiny();
        let t = TestDir::new("native-fallback");

        // Others can write the directory: nothing read or written there, nothing decompressed.
        set_mode(&t.0, 0o777);
        assert!(matches!(load(&engine, &digest, &compressed, &t.0), Err(None)));
        assert_eq!(std::fs::read_dir(&t.0).unwrap().count(), 0);

        // It cannot be written (unless the tests run as root): the code decompressed for the file
        // is handed back.
        set_mode(&t.0, 0o500);
        if std::fs::write(t.0.join("probe"), b"").is_err() {
            assert!(matches!(load(&engine, &digest, &compressed, &t.0), Err(Some(n)) if n == native));
        }
        set_mode(&t.0, 0o700);
    }

    #[cfg(feature = "compiler")]
    #[test]
    fn an_engine_that_cannot_be_mapped_is_not_written_again() {
        let (engine, native, digest, compressed) = tiny();
        // Loaded by an engine configured otherwise, failing as a mount that refuses executable
        // mappings would: the file checks out, mapping it does not.
        let mut cfg = crate::engine::config();
        cfg.wasm_threads(false).shared_memory(false);
        let other = Engine::new(&cfg).unwrap();
        let t = TestDir::new("native-unusable");
        assert!(matches!(load(&other, &digest, &compressed, &t.0), Err(Some(n)) if n == native));
        let unusable = t.0.join(format!("{}.unusable", hex(&digest)));
        assert!(unusable.exists() && !t.0.join(format!("{}.cwasm", hex(&digest))).exists());
        // Later starts skip the cache before decompressing anything.
        assert!(matches!(load(&other, &digest, &compressed, &t.0), Err(None)));
        std::fs::remove_file(&unusable).unwrap();
        assert!(load(&engine, &digest, &compressed, &t.0).is_ok());
    }
}
