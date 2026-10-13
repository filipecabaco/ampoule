//! `ampoule pack`: turn a release directory into a binary.
//!
//! The release is pruned (sources, docs, host-native code and build leftovers dropped), every
//! .beam loses its debug chunks, and the rest becomes a rootfs image (see `vfs`) that is appended
//! to a runtime stub together with the manifest:
//!
//! ```text
//! [stub][rootfs image][engine?][manifest JSON][rootfs len][engine len][manifest len]["AMPOULE1"]
//! ```
//!
//! The engine section is present when the release has NIFs: beam.wasm relinked with them and
//! precompiled for the binary's target (see `nif`).

use std::collections::BTreeSet;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::{Manifest, ReleaseInfo};

const MAGIC: &[u8; 8] = b"AMPOULE1";
const TRAILER: usize = 32;

/// OTP applications: their native code is either linked into beam.wasm (crypto, asn1) or not
/// needed, so it is dropped silently. Native code anywhere else stops the pack.
const OTP_APPS: &[&str] = &[
    "asn1",
    "common_test",
    "compiler",
    "crypto",
    "debugger",
    "dialyzer",
    "diameter",
    "edoc",
    "eldap",
    "erl_interface",
    "erts",
    "et",
    "eunit",
    "ftp",
    "inets",
    "jinterface",
    "kernel",
    "megaco",
    "mnesia",
    "observer",
    "odbc",
    "os_mon",
    "parsetools",
    "public_key",
    "reltool",
    "runtime_tools",
    "sasl",
    "snmp",
    "ssh",
    "ssl",
    "stdlib",
    "syntax_tools",
    "tftp",
    "tools",
    "wx",
    "xmerl",
];
const DROPPED_LIB_DIRS: &[&str] = &["src", "include", "doc", "docs", "c_src", "examples"];
const DROPPED_DIRS: &[&str] = &["plts", ".expert", ".elixir_ls", ".lexical", ".git"];
const DROPPED_EXTS: &[&str] = &["map", "gz", "br", "so", "dylib", "dll", "bat", "sh"];
const DROPPED_CHUNKS: &[&[u8; 4]] = &[b"Dbgi", b"Docs", b"ExCk", b"ExDp", b"Abst"];

pub struct Payload {
    pub rootfs: &'static [u8],
    /// A precompiled engine with the app's NIFs, replacing the built-in one.
    pub engine: Option<&'static [u8]>,
    pub manifest: Manifest,
}

/// Split a binary into its stub and, if it carries one, its payload.
pub fn split(bytes: &'static [u8]) -> Result<(&'static [u8], Option<Payload>)> {
    if bytes.len() < TRAILER || &bytes[bytes.len() - 8..] != MAGIC {
        return Ok((bytes, None));
    }
    let t = bytes.len() - TRAILER;
    let field = |i: usize| -> Result<usize> { Ok(u64::from_le_bytes(bytes[t + i * 8..t + i * 8 + 8].try_into()?) as usize) };
    let (rootfs_len, engine_len, manifest_len) = (field(0)?, field(1)?, field(2)?);
    let total = rootfs_len.checked_add(engine_len).and_then(|n| n.checked_add(manifest_len)).context("corrupt payload trailer")?;
    let start = t.checked_sub(total).context("corrupt payload trailer")?;
    let rootfs = &bytes[start..start + rootfs_len];
    let engine = (engine_len > 0).then(|| &bytes[start + rootfs_len..start + rootfs_len + engine_len]);
    let manifest: Manifest = serde_json::from_slice(&bytes[start + rootfs_len + engine_len..t]).context("corrupt payload manifest")?;
    if manifest.format != Manifest::FORMAT {
        bail!("payload format {} is not supported by this runtime (expects {})", manifest.format, Manifest::FORMAT);
    }
    Ok((&bytes[..start], Some(Payload { rootfs, engine, manifest })))
}

/// The running executable, read once.
pub fn self_bytes() -> Result<&'static [u8]> {
    let exe = std::env::current_exe()?;
    Ok(Box::leak(std::fs::read(&exe).with_context(|| format!("reading {}", exe.display()))?.into_boxed_slice()))
}

/// Prune, strip and compress `release` into a rootfs image. Native files are left out; the
/// ones outside OTP are listed in the stats for the caller to rebuild for wasm or refuse.
pub fn build_rootfs(release: &Path) -> Result<(Vec<u8>, Stats)> {
    let mut entries = Vec::new();
    let mut stats = Stats::default();
    walk(release, release, &mut entries, &mut stats)?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));

    let compressed: Vec<Vec<u8>> = parallel_compress(&entries);
    let mut index = Vec::new();
    let mut data = Vec::new();
    index.extend_from_slice(crate::vfs::MAGIC);
    index.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (e, stored) in entries.iter().zip(&compressed) {
        index.extend_from_slice(&(e.path.len() as u16).to_le_bytes());
        index.extend_from_slice(e.path.as_bytes());
        index.push(e.kind);
        index.extend_from_slice(&e.mode.to_le_bytes());
        index.extend_from_slice(&(e.data.len() as u64).to_le_bytes());
        index.extend_from_slice(&(data.len() as u64).to_le_bytes());
        index.extend_from_slice(&(stored.len() as u32).to_le_bytes());
        data.extend_from_slice(stored);
        if e.kind == crate::vfs::KIND_FILE {
            stats.files += 1;
            stats.bytes += e.data.len() as u64;
        }
    }
    index.extend_from_slice(&data);
    Ok((index, stats))
}

/// Write `stub` + payload to `out`, executable.
pub fn write_binary(stub: &[u8], rootfs: &[u8], engine: Option<&[u8]>, manifest: &Manifest, out: &Path) -> Result<()> {
    let manifest = serde_json::to_vec(manifest)?;
    let engine = engine.unwrap_or(&[]);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut bytes = Vec::with_capacity(stub.len() + rootfs.len() + engine.len() + manifest.len() + TRAILER);
    bytes.extend_from_slice(stub);
    bytes.extend_from_slice(rootfs);
    bytes.extend_from_slice(engine);
    bytes.extend_from_slice(&manifest);
    bytes.extend_from_slice(&(rootfs.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(engine.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(manifest.len() as u64).to_le_bytes());
    bytes.extend_from_slice(MAGIC);
    let tmp = out.with_extension("partial");
    std::fs::write(&tmp, bytes)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, out)?;
    Ok(())
}

/// Check a release can run on this runtime's ERTS: .beam files from a different OTP major will
/// not load.
pub fn check_erts(release: &ReleaseInfo, runtime_erts: &str) -> Result<()> {
    let major = |v: &str| v.split('.').next().unwrap_or("").to_string();
    if major(&release.erts_vsn) != major(runtime_erts) {
        bail!(
            "{} {} was built with ERTS {}, but this runtime carries ERTS {}: build the release with the matching OTP, or use a runtime for ERTS {}",
            release.name,
            release.vsn,
            release.erts_vsn,
            runtime_erts,
            release.erts_vsn
        );
    }
    Ok(())
}

#[derive(Default)]
pub struct Stats {
    pub files: usize,
    pub bytes: u64,
    pub dropped: usize,
    pub foreign_native: BTreeSet<String>,
}

struct Entry {
    path: String,
    kind: u8,
    mode: u32,
    data: Vec<u8>,
}

fn guest_path(root: &Path, p: &Path) -> String {
    let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
    if rel.is_empty() {
        "/".into()
    } else {
        format!("/{rel}")
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<Entry>, stats: &mut Stats) -> Result<()> {
    let rel = guest_path(root, dir);
    out.push(Entry { path: rel.clone(), kind: crate::vfs::KIND_DIR, mode: std::fs::metadata(dir)?.mode(), data: Vec::new() });
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)?.map(|e| e.map(|e| e.path())).collect::<std::io::Result<_>>()?;
    children.sort();
    for p in children {
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let path = guest_path(root, &p);
        let md = std::fs::symlink_metadata(&p)?;
        let ext = p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
        if is_native(&ext) {
            if let Some((app, _)) = lib_entry(&path) {
                if !OTP_APPS.contains(&app) {
                    stats.foreign_native.insert(path.clone());
                    // The NIF is linked into the engine; an empty file stays where the library
                    // was, for code that checks it exists before calling load_nif.
                    out.push(Entry { path: path.clone(), kind: crate::vfs::KIND_FILE, mode: md.mode(), data: Vec::new() });
                    continue;
                }
            }
        }
        if dropped(&rel, &path, &name, &ext, md.is_dir()) {
            stats.dropped += 1;
            continue;
        }
        if md.file_type().is_symlink() {
            let target = std::fs::read_link(&p)?.to_string_lossy().into_owned().into_bytes();
            out.push(Entry { path, kind: crate::vfs::KIND_SYMLINK, mode: md.mode(), data: target });
        } else if md.is_dir() {
            walk(root, &p, out, stats)?;
        } else {
            let mut data = std::fs::read(&p)?;
            if ext == "beam" {
                if let Some(stripped) = strip_beam(&data) {
                    data = stripped;
                }
            }
            out.push(Entry { path, kind: crate::vfs::KIND_FILE, mode: md.mode(), data });
        }
    }
    Ok(())
}

fn is_native(ext: &str) -> bool {
    matches!(ext, "so" | "dylib" | "dll")
}

/// `/lib/<app>-<vsn>/<rel>` -> (`<app>`, `<rel>`); `<rel>` is empty for the app directory itself.
pub fn lib_entry(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("/lib/")?;
    let (dir, rel) = rest.split_once('/').unwrap_or((rest, ""));
    Some((dir.rsplit_once('-').map_or(dir, |(app, _)| app), rel))
}

fn dropped(parent: &str, path: &str, name: &str, ext: &str, is_dir: bool) -> bool {
    if is_dir {
        return DROPPED_DIRS.contains(&name)
            || path == "/bin"
            || path == "/tmp"
            || (parent.starts_with("/lib/") && parent.matches('/').count() == 2 && DROPPED_LIB_DIRS.contains(&name))
            || (parent.starts_with("/erts-") && parent.matches('/').count() == 1 && name != "bin")
            || (parent.starts_with("/erts-") && parent.ends_with("/bin"));
    }
    if parent.starts_with("/erts-") {
        return true;
    }
    DROPPED_EXTS.contains(&ext)
}

/// Drop debug info, docs and type-checker chunks from a .beam ("FOR1" IFF container).
fn strip_beam(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 12 || &data[0..4] != b"FOR1" || &data[8..12] != b"BEAM" {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(b"FOR1\0\0\0\0BEAM");
    let mut i = 12;
    while i + 8 <= data.len() {
        let id: &[u8; 4] = data[i..i + 4].try_into().ok()?;
        let size = u32::from_be_bytes(data[i + 4..i + 8].try_into().ok()?) as usize;
        let padded = (size + 3) & !3;
        let chunk = data.get(i..i + 8 + padded).or_else(|| data.get(i..))?;
        if !DROPPED_CHUNKS.contains(&id) {
            out.extend_from_slice(chunk);
            out.resize(out.len() + (8 + padded - chunk.len()), 0);
        }
        i += 8 + padded;
    }
    let size = (out.len() - 8) as u32;
    out[4..8].copy_from_slice(&size.to_be_bytes());
    Some(out)
}

fn parallel_compress(entries: &[Entry]) -> Vec<Vec<u8>> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = entries.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = entries
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|e| if e.kind == crate::vfs::KIND_FILE { zstd::bulk::compress(&e.data, 19).expect("zstd") } else { e.data.clone() })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut c = id.to_vec();
        c.extend_from_slice(&(body.len() as u32).to_be_bytes());
        c.extend_from_slice(body);
        c.resize(c.len().next_multiple_of(4), 0);
        c
    }

    fn beam(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut b = b"FOR1".to_vec();
        b.extend_from_slice(&(body.len() as u32 + 4).to_be_bytes());
        b.extend_from_slice(b"BEAM");
        b.extend_from_slice(&body);
        b
    }

    #[test]
    fn strip_beam_drops_debug_chunks_and_fixes_the_size() {
        let code = chunk(b"Code", b"abcde");
        let stripped = strip_beam(&beam(&[code.clone(), chunk(b"Dbgi", b"debug info")])).unwrap();
        assert_eq!(stripped, beam(&[code]));
    }

    #[test]
    fn strip_beam_ignores_non_beam_files() {
        assert!(strip_beam(b"not a beam file").is_none());
    }

    #[test]
    fn lib_entry_splits_app_and_relative_path() {
        assert_eq!(lib_entry("/lib/my_app-1.2.3/priv/nif.so"), Some(("my_app", "priv/nif.so")));
    }

    #[test]
    fn lib_entry_keeps_dashes_in_app_names() {
        assert_eq!(lib_entry("/lib/foo-bar-0.1.0"), Some(("foo-bar", "")));
    }

    #[test]
    fn dropped_prunes_sources_but_not_ebin() {
        assert_eq!((dropped("/lib/x-1", "/lib/x-1/src", "src", "", true), dropped("/lib/x-1", "/lib/x-1/ebin", "ebin", "", true)), (true, false));
    }

    #[test]
    fn check_erts_compares_majors_only() {
        let rel = ReleaseInfo { name: "a".into(), vsn: "1".into(), erts_vsn: "16.4.0.5".into(), elixir: false };
        assert!(check_erts(&rel, "16.4.0.4").is_ok() && check_erts(&rel, "17.1").is_err());
    }

    #[test]
    fn rootfs_round_trips_through_vfs() {
        let dir = crate::TestDir::new("rootfs");
        let root = &dir.0;
        std::fs::create_dir_all(root.join("lib/app-1/ebin")).unwrap();
        std::fs::write(root.join("lib/app-1/ebin/app.app"), b"{application, app, []}.").unwrap();
        std::os::unix::fs::symlink("ebin/app.app", root.join("lib/app-1/link")).unwrap();
        let (image, stats) = build_rootfs(root).unwrap();
        let fs = crate::vfs::RootFs::parse(Box::leak(image.into_boxed_slice())).unwrap();
        let (resolved, node) = fs.lookup("/lib/app-1/link", true).unwrap();
        assert_eq!((resolved.as_str(), fs.read(node).unwrap(), stats.files), ("/lib/app-1/ebin/app.app", b"{application, app, []}.".to_vec(), 1));
    }

    #[test]
    fn a_truncated_rootfs_image_is_an_error_not_a_panic() {
        let dir = crate::TestDir::new("truncated");
        std::fs::create_dir_all(dir.0.join("lib/app-1")).unwrap();
        std::fs::write(dir.0.join("lib/app-1/f"), b"data").unwrap();
        let (image, _) = build_rootfs(&dir.0).unwrap();
        for cut in [4, 12, 30, image.len() - 1] {
            let blob: &'static [u8] = Box::leak(image[..cut].to_vec().into_boxed_slice());
            assert!(crate::vfs::RootFs::parse(blob).is_err(), "cut at {cut}");
        }
    }

    #[test]
    fn rootfs_lookup_follows_links_through_directories() {
        let dir = crate::TestDir::new("dirlink");
        std::fs::create_dir_all(dir.0.join("lib/app-1/priv")).unwrap();
        std::fs::write(dir.0.join("lib/app-1/priv/x"), b"x").unwrap();
        std::os::unix::fs::symlink("app-1", dir.0.join("lib/app")).unwrap();
        let (image, _) = build_rootfs(&dir.0).unwrap();
        let fs = crate::vfs::RootFs::parse(Box::leak(image.into_boxed_slice())).unwrap();
        assert_eq!(fs.lookup("/lib/app/priv/x", false).map(|(p, _)| p), Some("/lib/app-1/priv/x".to_string()));
    }

    #[test]
    fn write_binary_then_split_recovers_the_payload() {
        let rel = ReleaseInfo { name: "app".into(), vsn: "0.1.0".into(), erts_vsn: "16.4".into(), elixir: true };
        let manifest = Manifest::new(&rel, crate::config::Config::default());
        let dir = crate::TestDir::new("split");
        let out = dir.0.join("app");
        write_binary(b"STUB", b"ROOTFS", Some(b"ENGINE"), &manifest, &out).unwrap();
        let bytes: &'static [u8] = Box::leak(std::fs::read(&out).unwrap().into_boxed_slice());
        let (stub, payload) = split(bytes).unwrap();
        let p = payload.unwrap();
        assert_eq!((stub, p.rootfs, p.engine, p.manifest.name.as_str()), (&b"STUB"[..], &b"ROOTFS"[..], Some(&b"ENGINE"[..]), "app"));
    }

    #[test]
    fn split_without_a_trailer_is_just_a_stub() {
        let (stub, payload) = split(b"plain executable").unwrap();
        assert_eq!((stub, payload.is_none()), (&b"plain executable"[..], true));
    }
}
