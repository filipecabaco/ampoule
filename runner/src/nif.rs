//! Native code in a release (NIFs) rebuilt for wasm and linked into a per-app engine.
//!
//! Each dependency with native code is recompiled by Mix itself (`mix deps.compile DEP --force`;
//! for the project's own NIF, `mix compile --force` in a copy of the project)
//! with CC/CXX pointing at `ampoule-cc`/`ampoule-c++` (see `toolchain`), so whatever drives
//! its build (rebar3's port compiler, elixir_make, cmake) produces a wasm archive where it would
//! produce the shared library. This happens on private copies of the project's `deps/` and
//! `_build/<env>/` under `_build/ampoule-nif/`, so the project itself is left as it was.
//!
//! The archives are then registered in the static NIF table (ERTS finds a static NIF by the
//! module its init function declares, whatever path `load_nif` is given) and linked with the
//! engine kit into a new beam.wasm.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::toolchain::{Toolchain, WASM_TARGET};

pub struct NativeLib {
    pub app: String,
    /// Path inside the app directory, e.g. `priv/eiconv_nif.so`.
    pub rel_path: String,
    /// Unique C identifier: the NIF is compiled with STATIC_ERLANG_NIF_LIBNAME=<libname>.
    pub libname: String,
}

pub struct BuiltNif {
    pub libname: String,
    pub archive: PathBuf,
    pub cxx: bool,
    pub libs: Vec<String>,
    /// Built with Rustler: the engine needs the enif_* lookup table.
    pub rust: bool,
}

#[derive(Deserialize, Default)]
struct ArchiveMeta {
    cxx: bool,
    libs: Vec<String>,
}

#[derive(Deserialize)]
struct Kit {
    link_flags: Vec<String>,
    objects: Vec<String>,
    archives: Vec<String>,
    system_libs: Vec<String>,
    /// The LLVM the kit's objects were built with. They are LLVM bitcode (link-time optimisation),
    /// which only that LLVM can link. Absent in kits from before LTO.
    #[serde(default)]
    llvm_version: Option<String>,
}

/// A bitcode kit links only with the LLVM that built it; say so up front, not as a linker error.
fn check_llvm(kit: Option<&str>, sdk: Option<&str>) -> Result<()> {
    match (kit, sdk) {
        (Some(want), Some(have)) if want != have => bail!(
            "the engine kit was built with LLVM {want}, but this wasi-sdk has LLVM {have}: use the wasi-sdk the kit was built with (wasi-sdk 34 for this ampoule; AMPOULE_WASI_SDK)"
        ),
        (Some(want), None) => bail!("the engine kit needs LLVM {want}, and this wasi-sdk does not say which LLVM it has (no VERSION file)"),
        _ => Ok(()),
    }
}

/// `/lib/<app>-<vsn>/<rel>` paths of foreign native files, as NIFs to build.
pub fn natives(paths: &BTreeSet<String>) -> Vec<NativeLib> {
    paths
        .iter()
        .filter_map(|p| {
            let (app, rel) = crate::pack::lib_entry(p).filter(|(_, rel)| !rel.is_empty())?;
            let stem = Path::new(rel).file_stem()?.to_string_lossy().into_owned();
            let libname: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
            Some(NativeLib { app: app.to_string(), rel_path: rel.to_string(), libname })
        })
        .collect()
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    copy_tree_skipping(from, to, &|_| false)
}

/// `copy_tree`, leaving out directories whose name `skip` matches, at any depth.
fn copy_tree_skipping(from: &Path, to: &Path, skip: &dyn Fn(&str) -> bool) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let dest = to.join(entry.file_name());
        if ft.is_dir() && entry.file_name().to_str().is_some_and(skip) {
            continue;
        }
        if ft.is_dir() {
            copy_tree_skipping(&entry.path(), &dest, skip)?;
        } else if ft.is_symlink() {
            let target = std::fs::read_link(entry.path())?;
            let _ = std::os::unix::fs::symlink(target, &dest);
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

/// Where the project is copied to build its own NIF (see `copy_project`).
fn project_copy(project: &Path) -> PathBuf {
    project.join("_build/ampoule-nif/app")
}

/// Remove the copy of the project made to build its own NIF. It sits under _build, which CI
/// often caches, so it does not outlive the pack.
pub fn remove_project_copy(project: &Path) {
    let _ = std::fs::remove_dir_all(project_copy(project));
}

/// Top-level entries of the project left out of the copy its own NIF is built in:
/// - what the build recreates or reads from elsewhere (`_build`, `deps`, Rust's `target`);
/// - what it never needs and can be huge: tests, coverage, editor and language-server caches,
///   front-end assets and their node_modules (Realtime's `.expert` alone is 507 MB);
/// - tool-version files: mise refuses to run from an untrusted copy, and without them it finds
///   the project's own configuration one directory up;
/// - files that commonly hold secrets.
fn skip_in_copy(name: &str) -> bool {
    matches!(
        name,
        "_build"
            | "deps"
            | "target"
            | ".git"
            | ".elixir_ls"
            | ".lexical"
            | ".expert"
            | "test"
            | "cover"
            | "node_modules"
            | "assets"
            | "mise.toml"
            | ".mise.toml"
            | ".tool-versions"
            | ".envrc"
            | ".netrc"
    ) || name.starts_with(".env")
        || [".pem", ".key", ".p12", ".pfx", ".jks"].iter().any(|ext| name.ends_with(ext))
}

/// Copy the Mix project at `from` to `to` for building its own NIF (see `skip_in_copy`).
/// Symlinks are recreated, not followed, as in `copy_tree`.
fn copy_project(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_str().is_none_or(skip_in_copy) {
            continue;
        }
        let dest = to.join(&name);
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            let _ = std::os::unix::fs::symlink(std::fs::read_link(entry.path())?, &dest);
        } else if ft.is_dir() {
            // Build output nested in the project (native/<crate>/target, assets/node_modules).
            copy_tree_skipping(&entry.path(), &dest, &|n| matches!(n, "target" | "node_modules"))?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

/// The OTP application the Mix project at `project` defines (`app: :name` in its mix.exs), if
/// one can be read.
fn project_app(project: &Path) -> Option<String> {
    let src = std::fs::read_to_string(project.join("mix.exs")).ok()?;
    let at = src.find("app: :")? + "app: :".len();
    let name: String = src[at..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    (!name.is_empty()).then_some(name)
}

/// Remove host build products from a dependency so nothing compiled for the host is reused.
fn clean_host_artifacts(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if entry.file_type()?.is_dir() {
            if p.file_name().is_some_and(|n| n == "_build" || n == "target") {
                std::fs::remove_dir_all(&p)?;
            } else {
                clean_host_artifacts(&p)?;
            }
        } else if matches!(p.extension().and_then(|e| e.to_str()), Some("o" | "so" | "dylib" | "d" | "a")) {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

/// Source fixups for WASI, applied to the private copy of a native dependency. wasi-libc defines
/// `struct iovec` in its base types (it arrives with <stdint.h>), so a library's own fallback
/// definition is fenced off and the identical WASI one used instead.
fn apply_fixups(dir: &Path) -> Result<usize> {
    let mut fixed = 0;
    if !dir.is_dir() {
        return Ok(0);
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        // Not followed: a symlink in the copy can point at a file outside it (the original
        // project, or anywhere), which must not be patched, and can form a cycle.
        let ft = entry.file_type()?;
        let p = entry.path();
        if ft.is_dir() {
            fixed += apply_fixups(&p)?;
            continue;
        }
        if !ft.is_file() || !matches!(p.extension().and_then(|e| e.to_str()), Some("h" | "hpp" | "hh" | "c" | "cc" | "cpp")) {
            continue;
        }
        // Bytes, not text: a source in Latin-1 must get its fixup too, not be skipped.
        let src = std::fs::read(&p)?;
        // Only the amalgamation (a .c file) takes the SQLite stubs: sqlite3.h carries the same
        // WASI block, and definitions in a header would be duplicated wherever it is included.
        let is_c = p.extension().and_then(|e| e.to_str()) == Some("c");
        let patched = is_c.then(|| fixup_sqlite(&src)).flatten().or_else(|| fixup_iovec(&src));
        if let Some(patched) = patched {
            std::fs::write(&p, patched)?;
            fixed += 1;
        }
    }
    Ok(fixed)
}

/// Where `needle` first occurs in `hay`.
fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `ERL_FLAGS` for building `app`: the caller's own, plus elixir_make's `force_build` for `app`,
/// set as an application parameter. Dependencies with precompiled NIFs (cc_precompiler, as
/// exqlite uses) otherwise download a library built for the host instead of running `make`, and
/// leave nothing for wasm.
///
/// Only for names that are plain Erlang atoms: erlexec splits ERL_FLAGS on spaces and strips
/// quotes, so any other name would break the VM's start (`foo-bar`) or add flags of its own.
/// Such an app is built as before, without the override.
fn force_source_build(app: &str) -> String {
    let own = std::env::var("ERL_FLAGS").unwrap_or_default();
    let plain = app.starts_with(|c: char| c.is_ascii_lowercase()) && app.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !plain {
        return own;
    }
    format!("{own} -elixir_make force_build [{{{app},true}}]").trim_start().to_string()
}

fn fixup_iovec(src: &[u8]) -> Option<Vec<u8>> {
    let start = find(src, b"\nstruct iovec {")?;
    let end = start + find(&src[start..], b"};")? + 2;
    Some([&src[..start], b"\n#ifndef __wasi__", &src[start..end], b"\n#endif", &src[end..]].concat())
}

/// The SQLite amalgamation's WASI support, completed for a NIF. ampoule compiles NIFs with
/// `__linux__` defined, which turns SQLite's memory-mapped I/O on; under WASI SQLite has left out
/// <sys/mman.h>, so that code does not build: it is switched off instead (reads go through
/// read(), as with `PRAGMA mmap_size = 0`). Under WASI SQLite also defaults to single-threaded,
/// which a NIF called from several schedulers at once cannot be: unless the build chose, it is
/// thread-safe (wasm32-wasip1-threads has pthread mutexes). And WASI leaves out loading
/// extensions, which a wrapper NIF such as exqlite still calls: stubs report that it cannot be
/// done.
fn fixup_sqlite(src: &[u8]) -> Option<Vec<u8>> {
    const WASI: &[u8] = b"# define SQLITE_WASI 1\n";
    const MARK: &str = "ampoule: SQLite under WASI";
    if find(src, MARK.as_bytes()).is_some() || find(src, b"SQLITE_VERSION").is_none() {
        return None;
    }
    let at = find(src, WASI)? + WASI.len();
    let mmap = format!(
        "/* {MARK}: no memory-mapped I/O, and thread-safe. */\n# ifndef SQLITE_MAX_MMAP_SIZE\n#  define SQLITE_MAX_MMAP_SIZE 0\n# endif\n# ifndef SQLITE_THREADSAFE\n#  define SQLITE_THREADSAFE 1\n# endif\n"
    );
    let stubs = format!(
        r#"
/* {MARK}: there is no dynamic loading, so extensions cannot be loaded. */
#if defined(__wasi__) && defined(SQLITE_OMIT_LOAD_EXTENSION)
SQLITE_API int sqlite3_enable_load_extension(sqlite3 *db, int onoff){{
  (void)db;
  return onoff ? SQLITE_ERROR : SQLITE_OK;
}}
SQLITE_API int sqlite3_load_extension(sqlite3 *db, const char *zFile, const char *zProc, char **pzErrMsg){{
  (void)db; (void)zFile; (void)zProc;
  if( pzErrMsg ) *pzErrMsg = sqlite3_mprintf("extensions cannot be loaded under WASI");
  return SQLITE_ERROR;
}}
#endif
"#
    );
    Some([&src[..at], mmap.as_bytes(), &src[at..], stubs.as_bytes()].concat())
}

fn is_archive(p: &Path) -> bool {
    std::fs::read(p).map(|b| b.starts_with(b"!<arch>\n")).unwrap_or(false)
}

/// A Rust crate in the dependency (`native/<crate>/Cargo.toml`, Rustler's layout) that builds
/// the NIF `lib`: the one whose name the library file starts with, or the only one.
fn rust_crate(dep: &Path, lib: &NativeLib) -> Option<PathBuf> {
    let native = dep.join("native");
    let crates: Vec<PathBuf> = std::fs::read_dir(&native).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join("Cargo.toml").is_file()).collect();
    let stem = Path::new(&lib.rel_path).file_stem()?.to_string_lossy().trim_start_matches("lib").to_string();
    crates
        .iter()
        .find(|c| c.file_name().is_some_and(|n| stem.starts_with(&*n.to_string_lossy())))
        .cloned()
        .or_else(|| (crates.len() == 1).then(|| crates[0].clone()))
}

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
}

#[derive(Deserialize)]
struct CargoPackage {
    name: String,
    version: String,
    manifest_path: PathBuf,
}

/// Rustler on WASI: the enif_* API is linked statically and found through `aos_enif_lookup`
/// (generated by `enif_lookup`) instead of dlsym, and ErlNifEvent is a plain int.
fn patch_rustler(src: &Path, dest: &Path) -> Result<()> {
    let _ = std::fs::remove_dir_all(dest);
    copy_tree(src, dest)?;
    let filler = dest.join("src/sys/nif_filler.rs");
    let text = std::fs::read_to_string(&filler)?;
    let unix = "#[cfg(not(target_os = \"windows\"))]\nmod internal {";
    if !text.contains(unix) {
        bail!("unexpected rustler nif_filler.rs layout in {}", src.display());
    }
    let wasi = r#"#[cfg(target_os = "wasi")]
mod internal {
    use super::DynNifFiller;

    unsafe extern "C" {
        fn aos_enif_lookup(name: *const u8, len: usize) -> *const core::ffi::c_void;
    }

    pub(crate) struct StaticNifFiller;

    impl DynNifFiller for StaticNifFiller {
        fn write<T: Copy>(&self, field: &mut Option<T>, name: &str) {
            // The names are NUL-terminated, for dlsym.
            let name = name.trim_end_matches('\0');
            let addr = unsafe { aos_enif_lookup(name.as_ptr(), name.len()) };
            if !addr.is_null() {
                *field = Some(unsafe { core::mem::transmute_copy::<*const core::ffi::c_void, T>(&addr) });
            }
        }
    }

    pub(crate) fn new() -> impl DynNifFiller {
        StaticNifFiller
    }
}

#[cfg(all(not(target_os = "windows"), not(target_os = "wasi")))]
mod internal {"#;
    std::fs::write(&filler, text.replacen(unix, wasi, 1))?;
    let types = dest.join("src/sys/types.rs");
    let text = std::fs::read_to_string(&types)?;
    let marker = "#[cfg(unix)]\npub type ErlNifEvent = os::unix::io::RawFd;";
    if !text.contains(marker) {
        bail!("unexpected rustler types.rs layout in {}", src.display());
    }
    std::fs::write(&types, text.replacen(marker, &format!("{marker}\n\n#[cfg(target_os = \"wasi\")]\npub type ErlNifEvent = std::os::raw::c_int;"), 1))?;
    Ok(())
}

/// Crates that need source changes to build for WASI.
fn wasi_patch(name: &str) -> Option<fn(&Path, &Path) -> Result<()>> {
    match name {
        "rustler" => Some(patch_rustler),
        _ => None,
    }
}

fn build_rust(app: &str, krate: &Path, lib: &NativeLib, tc: &Toolchain, work: &Path) -> Result<BuiltNif> {
    eprintln!("==> building {app} for wasm (Rust crate {}, {})", krate.file_name().unwrap().to_string_lossy(), lib.rel_path);
    let _ = std::fs::remove_dir_all(krate.join("target"));
    let output = Command::new("cargo").args(["metadata", "--format-version", "1"]).current_dir(krate).output().context("running cargo metadata")?;
    if !output.status.success() {
        bail!("cargo metadata failed for {app}: {}", String::from_utf8_lossy(&output.stderr));
    }
    let meta: CargoMetadata = serde_json::from_slice(&output.stdout)?;
    if !meta.packages.iter().any(|p| p.name == "rustler") {
        bail!("{app}: no rustler in the crate's dependencies");
    }
    let manifest = krate.join("Cargo.toml");
    let mut toml = std::fs::read_to_string(&manifest)?;
    if toml.contains("[patch.crates-io]") {
        bail!("{app}: its Cargo.toml already has a [patch.crates-io] section");
    }
    toml.push_str("\n[patch.crates-io]\n");
    for pkg in &meta.packages {
        if let Some(patch) = wasi_patch(&pkg.name) {
            let patched = work.join("patches").join(format!("{}-{}", pkg.name, pkg.version));
            patch(pkg.manifest_path.parent().unwrap(), &patched)?;
            toml.push_str(&format!("{} = {{ path = {:?} }}\n", pkg.name, patched.display().to_string()));
        }
    }
    std::fs::write(&manifest, toml)?;

    let target_env = WASM_TARGET.replace('-', "_");
    let cflags = format!(
        "--target={WASM_TARGET} -pthread -D_GNU_SOURCE -D_WASI_EMULATED_SIGNAL -D_WASI_EMULATED_PROCESS_CLOCKS -D_WASI_EMULATED_MMAN -D_WASI_EMULATED_GETPID -isystem {}/ampoule-os/compat",
        tc.kit.display()
    );
    let status = Command::new("cargo")
        .args(["rustc", "--release", "--target", WASM_TARGET, "--crate-type", "staticlib"])
        .current_dir(krate)
        .env(format!("CC_{target_env}"), tc.clang(false))
        .env(format!("CXX_{target_env}"), tc.clang(true))
        .env(format!("AR_{target_env}"), tc.ar())
        .env(format!("CFLAGS_{target_env}"), &cflags)
        .env(format!("CXXFLAGS_{target_env}"), &cflags)
        .status()
        .context("running cargo")?;
    if !status.success() {
        bail!("building the Rust crate of {app} for wasm failed: {status}");
    }
    let staticlib = std::fs::read_dir(krate.join("target").join(WASM_TARGET).join("release"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "a"))
        .with_context(|| format!("{app}: cargo produced no static library"))?;

    // ERTS calls <libname>_nif_init; Rustler exports nif_init.
    let shim_src = work.join(format!("{}_init.c", lib.libname));
    let shim_obj = work.join(format!("{}_init.o", lib.libname));
    std::fs::write(&shim_src, format!("void *nif_init(void);\nvoid *{}_nif_init(void) {{ return nif_init(); }}\n", lib.libname))?;
    tc.compile_object(&shim_src, &shim_obj, &[])?;
    let archive = work.join(format!("lib{}.a", lib.libname));
    tc.archive(&archive, &[staticlib], &[shim_obj]).with_context(|| format!("archiving the Rust NIF of {app}"))?;
    Ok(BuiltNif { libname: lib.libname.clone(), archive, cxx: false, libs: Vec::new(), rust: true })
}

/// Rebuild the native dependencies of the release built from `project` for wasm.
pub fn build(project: &Path, mix_env: &str, natives: &[NativeLib], tc: &Toolchain) -> Result<Vec<BuiltNif>> {
    let work = project.join("_build/ampoule-nif");
    let deps = work.join("deps");
    let build = work.join(mix_env);
    let bin = work.join("bin");
    let _ = std::fs::remove_dir_all(&work);
    copy_tree(&project.join("deps"), &deps).context("copying deps/ (is this the release's Mix project?)")?;
    // Only lib/: the rest of _build/<env> is earlier releases and packed binaries, which the
    // build does not read and which can be most of its size.
    copy_tree(&project.join("_build").join(mix_env).join("lib"), &build.join("lib"))?;
    std::fs::create_dir_all(&bin)?;
    let me = std::env::current_exe()?;
    for name in ["ampoule-cc", "ampoule-c++"] {
        std::os::unix::fs::symlink(&me, bin.join(name))?;
    }
    // Build scripts that pick their sources with `uname -s` should pick Linux's.
    let uname = bin.join("uname");
    std::fs::write(&uname, "#!/bin/sh\ncase \"$1\" in -s) echo Linux ;; -m) echo wasm32 ;; *) exec /usr/bin/uname \"$@\" ;; esac\n")?;
    std::fs::set_permissions(&uname, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());

    let mut by_app: BTreeMap<&str, Vec<&NativeLib>> = BTreeMap::new();
    for n in natives {
        by_app.entry(&n.app).or_default().push(n);
    }
    let mut built = Vec::new();
    for (app, libs) in by_app {
        if libs.len() > 1 {
            bail!(
                "{app} has several native libraries ({}); building more than one NIF per dependency is not supported yet",
                libs.iter().map(|l| l.rel_path.as_str()).collect::<Vec<_>>().join(", ")
            );
        }
        let lib = libs[0];
        // A NIF of the project's own app is built from a copy of the project: Rust from its
        // native/, C and C++ by its own compilers (elixir_make and a Makefile over c_src/, say).
        let own = !deps.join(app).is_dir();
        if own && project_app(project).as_deref() != Some(app) {
            // Neither under deps/ nor the project's own app: a path dependency, whose project is
            // elsewhere. Building it from a copy of this project would build the wrong thing.
            bail!("{app} has a native library ({}), but it is neither the project's own app nor under deps/ (a path dependency?); NIFs in path dependencies are not supported yet", lib.rel_path);
        }
        let dep_dir = if own {
            let dir = project_copy(project).join(app);
            copy_project(project, &dir)?;
            dir
        } else {
            deps.join(app)
        };
        clean_host_artifacts(&dep_dir)?;
        apply_fixups(&dep_dir)?;
        if let Some(krate) = rust_crate(&dep_dir, lib) {
            built.push(build_rust(app, &krate, lib, tc, &work)?);
            continue;
        }
        let _ = std::fs::remove_dir_all(build.join("lib").join(app));
        eprintln!("==> building {app} for wasm ({})", lib.rel_path);
        let mut mix = Command::new("mix");
        if own {
            mix.args(["compile", "--force"]).current_dir(&dep_dir);
        } else {
            mix.args(["deps.compile", app, "--force"]).current_dir(project);
        }
        let status = mix
            .env("MIX_ENV", mix_env)
            .env("PATH", &path)
            .env("MIX_DEPS_PATH", &deps)
            .env("MIX_BUILD_PATH", &build)
            .env("CC", bin.join("ampoule-cc"))
            .env("CXX", bin.join("ampoule-c++"))
            .env("AR", tc.ar())
            .env("RANLIB", tc.wasi_sdk.join("bin/llvm-ranlib"))
            .env("AMPOULE_NIF_LIBNAME", &lib.libname)
            .env("ERL_FLAGS", force_source_build(app))
            .env("AMPOULE_WASI_SDK", &tc.wasi_sdk)
            .env("AMPOULE_KIT", &tc.kit)
            .status()
            .context("running mix")?;
        if !status.success() {
            bail!("building {app} for wasm failed: {status}");
        }
        let candidates = [build.join("lib").join(app).join(&lib.rel_path), deps.join(app).join(&lib.rel_path)];
        let archive = candidates
            .iter()
            .find(|p| is_archive(p))
            .cloned()
            .with_context(|| format!("{app}: no wasm archive at {} after the build (does its build ignore CC?)", lib.rel_path))?;
        let meta_path = candidates.iter().map(|p| PathBuf::from(format!("{}.ampoule.json", p.display()))).find(|p| p.exists());
        let meta: ArchiveMeta = meta_path.and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        built.push(BuiltNif { libname: lib.libname.clone(), archive, cxx: meta.cxx, libs: meta.libs, rust: false });
    }
    Ok(built)
}

/// Libraries a host link asks for that the wasm link must not (they are in wasi-libc or have no
/// wasm equivalent).
fn provided(lib: &str) -> bool {
    matches!(lib, "c" | "m" | "dl" | "pthread" | "rt" | "iconv" | "stdc++" | "c++" | "gcc" | "gcc_s" | "util" | "erl_interface" | "ei")
}

/// `aos_enif_lookup`, for Rustler NIFs: the enif_* functions the kit's erl_nif_api_funcs.h
/// declares, by name. The names are taken from the header through the preprocessor, so the
/// table follows whatever ERTS the kit carries.
fn enif_lookup(tc: &Toolchain, work: &Path) -> Result<PathBuf> {
    let probe = work.join("enif_names.c");
    std::fs::write(&probe, "#define STATIC_ERLANG_NIF 1\n#include \"erl_nif.h\"\n#undef ERL_NIF_API_FUNC_DECL\n#define ERL_NIF_API_FUNC_DECL(RET, NAME, ARGS) AMPOULE_ENIF NAME AMPOULE_END\n#include \"erl_nif_api_funcs.h\"\n")?;
    let out = Command::new(tc.clang(false))
        .args([format!("--target={WASM_TARGET}"), "-E".into(), "-P".into(), format!("-I{}", tc.kit.join("include").display())])
        .arg(&probe)
        .output()?;
    if !out.status.success() {
        bail!("listing the enif API failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let names: BTreeSet<&str> =
        text.split("AMPOULE_ENIF").skip(1).filter_map(|t| t.split("AMPOULE_END").next()).map(str::trim).filter(|n| n.starts_with("enif_")).collect();
    let mut src =
        String::from("#define STATIC_ERLANG_NIF 1\n#include <string.h>\n#include \"erl_nif.h\"\nconst void *aos_enif_lookup(const char *name, size_t len) {\n");
    for n in &names {
        src.push_str(&format!("    if (len == {} && memcmp(name, \"{n}\", len) == 0) return (const void *)&{n};\n", n.len()));
    }
    src.push_str("    return NULL;\n}\n");
    let c = work.join("enif_lookup.c");
    let o = work.join("enif_lookup.o");
    std::fs::write(&c, src)?;
    tc.compile_object(&c, &o, &[format!("-I{}", tc.kit.join("include").display())])?;
    Ok(o)
}

/// Link beam.wasm from the engine kit plus `nifs`.
pub fn link_engine(tc: &Toolchain, nifs: &[BuiltNif], work: &Path) -> Result<Vec<u8>> {
    let kit: Kit = serde_json::from_slice(&std::fs::read(tc.kit.join("kit.json"))?).context("reading kit.json")?;
    check_llvm(kit.llvm_version.as_deref(), tc.llvm_version().as_deref())?;
    std::fs::create_dir_all(work)?;

    let table = std::fs::read_to_string(tc.kit.join("driver_tab.i"))?;
    let marker = "ErtsStaticNif erts_static_nif_tab[] =\n{\n";
    if !table.contains(marker) {
        bail!("engine kit: unexpected driver_tab.i");
    }
    let decls: String = nifs.iter().map(|n| format!("void *{}_nif_init(void);\n", n.libname)).collect();
    let entries: String = nifs.iter().map(|n| format!("    {{&{}_nif_init, 0, (0x0), ((void*)0)}},\n", n.libname)).collect();
    let table = table.replacen(marker, &format!("{decls}{marker}{entries}"), 1);
    let table_src = work.join("driver_tab.i");
    let table_obj = work.join("driver_tab.o");
    std::fs::write(&table_src, table)?;
    tc.compile_object(&table_src, &table_obj, &[])?;

    let cxx = nifs.iter().any(|n| n.cxx);
    let out = work.join("beam.wasm");
    let mut link = Command::new(tc.clang(cxx));
    link.arg(format!("--target={WASM_TARGET}")).arg("-o").arg(&out).args(&kit.link_flags);
    link.args(kit.objects.iter().map(|o| tc.kit.join(o))).arg(&table_obj);
    if nifs.iter().any(|n| n.rust) {
        link.arg(enif_lookup(tc, work)?);
    }
    link.args(nifs.iter().map(|n| &n.archive));
    link.args(kit.archives.iter().map(|a| tc.kit.join(a)));
    let extra: BTreeSet<&str> = nifs.iter().flat_map(|n| n.libs.iter().map(String::as_str)).filter(|l| !provided(l)).collect();
    link.args(extra.iter().map(|l| format!("-l{l}")));
    if cxx {
        link.arg("-nostdlib++").args(tc.eh_cxx_libs());
    }
    link.arg("-lsetjmp");
    link.args(&kit.system_libs);
    let output = link.output().context("running the wasm linker")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let shown: Vec<&str> = stderr.lines().filter(|l| !l.contains("unknown -z value") && !l.contains("argument unused")).take(30).collect();
        bail!("linking the engine failed:\n{}", shown.join("\n"));
    }
    Ok(std::fs::read(out)?)
}

/// Rust target triple of an ampoule target name, for precompiling.
#[cfg(feature = "compiler")]
fn triple(target: &str) -> Result<&'static str> {
    Ok(match target {
        "aarch64-darwin" => "aarch64-apple-darwin",
        "x86_64-darwin" => "x86_64-apple-darwin",
        "aarch64-linux" => "aarch64-unknown-linux-musl",
        "x86_64-linux" => "x86_64-unknown-linux-musl",
        other => bail!("unknown target {other}"),
    })
}

/// beam.wasm compiled to native code for `target`, compressed as the runtime expects.
#[cfg(feature = "compiler")]
pub fn precompile(wasm: &[u8], target: &str) -> Result<Vec<u8>> {
    let cfg = crate::engine_target::for_target(triple(target)?)?;
    let engine = wasmtime::Engine::new(&cfg)?;
    let native = engine.precompile_module(wasm).map_err(|e| anyhow::anyhow!("precompiling the engine for {target}: {e:?}"))?;
    // As build.rs compresses the built-in engine: smaller, and no slower to load.
    Ok(zstd::bulk::compress(&native, 19)?)
}

#[cfg(not(feature = "compiler"))]
pub fn precompile(_wasm: &[u8], target: &str) -> Result<Vec<u8>> {
    bail!("this ampoule cannot precompile an engine with NIFs for {target}: it was built without the `compiler` feature; pack with a release's `ampoule`")
}

#[cfg(test)]
mod tests {
    use super::{check_llvm, copy_project, find, fixup_iovec, fixup_sqlite, force_source_build, project_app, skip_in_copy};

    #[test]
    fn a_bitcode_kit_links_only_with_its_own_llvm() {
        assert!(check_llvm(Some("23.1.0"), Some("23.1.0")).is_ok());
        assert!(check_llvm(Some("23.1.0"), Some("22.1.0")).is_err());
        assert!(check_llvm(Some("23.1.0"), None).is_err());
        // Kits from before LTO name no LLVM and link with any.
        assert!(check_llvm(None, Some("22.1.0")).is_ok());
    }

    #[test]
    fn the_project_copy_leaves_out_secrets_assets_and_build_output() {
        for name in [
            "_build",
            "deps",
            "target",
            ".git",
            ".expert",
            "test",
            "cover",
            "node_modules",
            "assets",
            "mise.toml",
            ".tool-versions",
            ".env",
            ".env.prod",
            ".envrc",
            "server.pem",
            "id.key",
        ] {
            assert!(skip_in_copy(name), "{name} should not be copied");
        }
        for name in ["mix.exs", "mix.lock", "lib", "c_src", "native", "Makefile", "config", "priv", ".formatter.exs"] {
            assert!(!skip_in_copy(name), "{name} should be copied");
        }
    }

    #[test]
    fn the_project_copy_recreates_symlinks_instead_of_following_them() {
        let dir = crate::TestDir::new("copy-project");
        let (src, dst) = (dir.0.join("src"), dir.0.join("dst"));
        std::fs::create_dir_all(src.join("c_src")).unwrap();
        std::fs::create_dir_all(src.join("native/crate/target/debug")).unwrap();
        std::fs::write(src.join("native/crate/Cargo.toml"), "").unwrap();
        std::fs::write(src.join("mix.exs"), "defmodule M do\n  def project, do: [app: :checksum]\nend\n").unwrap();
        std::fs::write(src.join(".env"), "SECRET=1").unwrap();
        std::os::unix::fs::symlink("/nonexistent/target", src.join(".#mix.exs")).unwrap();
        copy_project(&src, &dst).unwrap();
        assert!(dst.join("mix.exs").is_file() && dst.join("c_src").is_dir() && !dst.join(".env").exists());
        assert!(dst.join("native/crate/Cargo.toml").is_file() && !dst.join("native/crate/target").exists());
        assert_eq!(std::fs::read_link(dst.join(".#mix.exs")).unwrap(), std::path::Path::new("/nonexistent/target"));
        assert_eq!(project_app(&src).as_deref(), Some("checksum"));
    }

    #[test]
    fn sqlite_under_wasi_gets_no_mmap_threads_and_extension_stubs_once() {
        let amalgamation: &[u8] = b"#define SQLITE_VERSION \"3.54.1\"\n#if defined(__wasi__)\n# undef SQLITE_WASI\n# define SQLITE_WASI 1\n#endif\nint body;\n";
        let patched = fixup_sqlite(amalgamation).unwrap();
        let text = String::from_utf8(patched.clone()).unwrap();
        let wasi = text.find("# define SQLITE_WASI 1").unwrap();
        assert!(text.find("#  define SQLITE_MAX_MMAP_SIZE 0").unwrap() > wasi);
        assert!(text.find("#  define SQLITE_THREADSAFE 1").unwrap() > wasi);
        assert!(text.ends_with("#endif\n") && text.contains("sqlite3_enable_load_extension"));
        assert_eq!(fixup_sqlite(&patched), None);
        assert_eq!(fixup_sqlite(b"int unrelated;\n"), None);
    }

    #[test]
    fn fixups_work_on_sources_that_are_not_utf8() {
        // Latin-1 in a comment: the old text-based fixups skipped such a file.
        let src: &[u8] = b"/* caf\xe9 */\nstruct iovec { void *b; int n; };\nint x;\n";
        let patched = fixup_iovec(src).unwrap();
        assert!(find(&patched, b"#ifndef __wasi__").is_some() && find(&patched, b"caf\xe9").is_some());
    }

    #[test]
    fn force_build_is_set_only_for_plain_atom_app_names() {
        assert!(force_source_build("exqlite").ends_with("-elixir_make force_build [{exqlite,true}]"));
        assert!(!force_source_build("foo-bar").contains("force_build"));
        assert!(!force_source_build("x',true}] -eval halt() -y [{'").contains("-eval"));
    }
}
