//! The wasm toolchain used to build NIFs: wasi-sdk and the engine kit (see scripts/engine.exs), and
//! `ampoule cc` / `ampoule c++`, the compiler wrapper NIF build systems are pointed at.
//!
//! The wrapper compiles for wasm32-wasip1-threads with the kit's erl_nif.h (32-bit, static NIF
//! API) and the ampoule-os header, drops host-only flags, and turns a shared-library link
//! (`-shared`/`-bundle` ... `-o x.so`) into a static archive at the same path. Build systems that
//! expect to produce `priv/x.so` therefore produce a wasm archive there instead, which
//! `ampoule pack` links into the engine.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

pub const WASM_TARGET: &str = "wasm32-wasip1-threads";

pub struct Toolchain {
    pub wasi_sdk: PathBuf,
    pub kit: PathBuf,
}

fn first_existing(candidates: Vec<Option<PathBuf>>) -> Option<PathBuf> {
    candidates.into_iter().flatten().find(|p| p.is_dir())
}

impl Toolchain {
    /// AMPOULE_WASI_SDK / AMPOULE_KIT, else next to this executable (`kit/`, `../wasi-sdk`),
    /// else ~/.cache/ampoule.
    pub fn locate() -> Result<Toolchain> {
        let exe_dir = std::env::current_exe()?.parent().map(Path::to_path_buf).unwrap_or_default();
        let cache = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/ampoule"));
        let wasi_sdk = first_existing(vec![
            std::env::var_os("AMPOULE_WASI_SDK").map(PathBuf::from),
            Some(exe_dir.join("wasi-sdk")),
            Some(exe_dir.join("../wasi-sdk")),
            cache.as_ref().map(|c| c.join("wasi-sdk")),
        ])
        .context("wasi-sdk not found: set AMPOULE_WASI_SDK (https://github.com/WebAssembly/wasi-sdk/releases)")?;
        let kit = first_existing(vec![
            std::env::var_os("AMPOULE_KIT").map(PathBuf::from),
            Some(exe_dir.join("kit")),
            cache.as_ref().map(|c| c.join(format!("kit-{}", crate::ERTS_VSN))),
        ])
        .context("engine kit not found: put the kit/ directory from an ampoule release next to the binary, or set AMPOULE_KIT (scripts/engine.exs builds it into build/engine/kit)")?;
        Ok(Toolchain { wasi_sdk: wasi_sdk.canonicalize()?, kit: kit.canonicalize()? })
    }

    pub fn clang(&self, cxx: bool) -> PathBuf {
        self.wasi_sdk.join("bin").join(if cxx { "clang++" } else { "clang" })
    }

    /// The exception-handling builds of libc++, libc++abi and libunwind.
    pub fn eh_cxx_libs(&self) -> Vec<PathBuf> {
        let dir = self.wasi_sdk.join("share/wasi-sysroot/lib").join(WASM_TARGET).join("eh");
        ["libc++.a", "libc++abi.a", "libunwind.a"].iter().map(|l| dir.join(l)).collect()
    }

    /// The LLVM this wasi-sdk ships, from its VERSION file ("llvm-version: 23.1.0").
    pub fn llvm_version(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.wasi_sdk.join("VERSION")).ok()?;
        text.lines().find_map(|l| l.strip_prefix("llvm-version:")).map(|v| v.trim().to_string())
    }

    pub fn ar(&self) -> PathBuf {
        self.wasi_sdk.join("bin/llvm-ar")
    }

    /// Create the archive `out` from the members of `archives` plus `objects` (an llvm-ar MRI
    /// script, so archives are merged rather than nested).
    pub fn archive(&self, out: &Path, archives: &[PathBuf], objects: &[PathBuf]) -> Result<()> {
        use std::io::Write;
        let _ = std::fs::remove_file(out);
        let mut script = format!("create {}\n", out.display());
        for a in archives {
            script.push_str(&format!("addlib {}\n", a.display()));
        }
        for o in objects {
            script.push_str(&format!("addmod {}\n", o.display()));
        }
        script.push_str("save\nend\n");
        let mut ar = Command::new(self.ar()).arg("-M").stdin(std::process::Stdio::piped()).spawn().context("running llvm-ar")?;
        ar.stdin.take().context("llvm-ar stdin")?.write_all(script.as_bytes())?;
        if !ar.wait()?.success() {
            bail!("llvm-ar failed for {}", out.display());
        }
        Ok(())
    }

    /// Compile one C source of the runtime's own (NIF table, init shims) to a wasm object.
    pub fn compile_object(&self, src: &Path, obj: &Path, extra: &[String]) -> Result<()> {
        let status = Command::new(self.clang(false))
            .args([format!("--target={WASM_TARGET}"), "-O2".into(), "-pthread".into(), "-w".into()])
            .args(extra)
            .arg("-c")
            .arg(src)
            .arg("-o")
            .arg(obj)
            .status()
            .context("running wasi-sdk clang")?;
        if !status.success() {
            bail!("compiling {} failed", src.display());
        }
        Ok(())
    }

    /// Flags every NIF compile gets, ahead of the build system's own (so the kit's headers win).
    pub fn compile_flags(&self) -> Vec<String> {
        let k = self.kit.display();
        let mut f: Vec<String> = vec![
            format!("--target={WASM_TARGET}"),
            "-pthread".into(),
            "-D_GNU_SOURCE".into(),
            // The guest is Linux as far as OTP and NIFs are concerned (uname says so too): Linux
            // code paths use /proc and POSIX calls the runtime provides.
            "-D__linux__".into(),
            "-D_WASI_EMULATED_MMAN".into(),
            "-D_WASI_EMULATED_SIGNAL".into(),
            "-D_WASI_EMULATED_PROCESS_CLOCKS".into(),
            "-D_WASI_EMULATED_GETPID".into(),
            format!("-isystem{k}/ampoule-os/compat"),
            "-include".into(),
            format!("{k}/ampoule-os/ampoule_os.h"),
            format!("-I{k}/include"),
            // setjmp/longjmp through wasm exception handling (the standardized, exnref flavour).
            "-mllvm".into(),
            "-wasm-enable-sjlj".into(),
            "-mllvm".into(),
            "-wasm-use-legacy-eh=false".into(),
        ];
        if let Ok(lib) = std::env::var("AMPOULE_NIF_LIBNAME") {
            f.push(format!("-DSTATIC_ERLANG_NIF_LIBNAME={lib}"));
        }
        f
    }
}

/// Host-only flags dropped from compiles and links.
fn host_only(arg: &str) -> bool {
    matches!(
        arg,
        "-shared" | "-bundle" | "-dynamiclib" | "-flat_namespace" | "-fPIE" | "-pie" | "-no-pie" | "-rdynamic" | "-static-libgcc" | "-static-libstdc++"
    ) || arg.starts_with("-mmacosx-version-min")
        || arg.starts_with("-march=")
        || arg.starts_with("-mcpu=")
        || arg.starts_with("-mtune=")
        || arg.starts_with("-Wl,")
        || arg.starts_with("-fuse-ld")
}

/// Flags whose next argument belongs to them and goes with them.
fn takes_value(arg: &str) -> bool {
    matches!(arg, "-arch" | "-undefined" | "-install_name" | "-framework" | "-isysroot" | "-target" | "-current_version" | "-compatibility_version")
}

/// An archive whose members are wasm objects (a host archive on a link line is skipped).
fn is_wasm_archive(p: &Path) -> bool {
    std::fs::read(p).map(|b| b.windows(8).any(|w| w == b"\0asm\x01\0\0\0")).unwrap_or(false)
}

/// `ampoule cc ARGS` / `ampoule c++ ARGS`.
pub fn cc_main(args: &[String], cxx: bool) -> Result<()> {
    let tc = Toolchain::locate()?;
    let compile_only = args.iter().any(|a| matches!(a.as_str(), "-c" | "-E" | "-S" | "-M" | "-MM"));
    let mut kept = Vec::new();
    let mut out: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if takes_value(a) {
            i += 2;
            continue;
        }
        if a == "-o" {
            out = args.get(i + 1).cloned();
            kept.push(a.clone());
            if let Some(o) = &out {
                kept.push(o.clone());
            }
            i += 2;
            continue;
        }
        if !host_only(a) {
            kept.push(a.clone());
        }
        i += 1;
    }

    if compile_only {
        let mut cmd = Command::new(tc.clang(cxx));
        cmd.args(tc.compile_flags());
        if cxx {
            // C++ exceptions through wasm exception handling, matching the EH libc++ the engine links.
            cmd.arg("-fwasm-exceptions");
        }
        let status = cmd.args(&kept).status().context("running wasi-sdk clang")?;
        std::process::exit(status.code().unwrap_or(1));
    }

    // A link: archive the objects instead (compiling any sources first). Static libraries it
    // links (`x.a`, or `-lname` found in a `-L` directory) are merged in, since the engine link
    // sees only this one archive.
    let out = out.context("ampoule cc: link without -o")?;
    let lib_dirs: Vec<PathBuf> = kept.iter().filter_map(|a| a.strip_prefix("-L")).filter(|d| !d.is_empty()).map(PathBuf::from).collect();
    let mut objects: Vec<PathBuf> = Vec::new();
    let mut archives: Vec<PathBuf> = Vec::new();
    let mut libs = Vec::new();
    for a in &kept {
        let p = Path::new(a);
        match p.extension().and_then(|e| e.to_str()) {
            Some("o" | "obj") => objects.push(p.to_path_buf()),
            Some("a") if is_wasm_archive(p) => archives.push(p.to_path_buf()),
            Some("a") => eprintln!("ampoule cc: skipping host archive {a}"),
            Some("c" | "cc" | "cpp" | "cxx" | "C" | "m") => {
                let obj = format!("{out}.{}.o", objects.len());
                let flags: Vec<&String> = kept
                    .iter()
                    .filter(|k| k.starts_with("-D") || k.starts_with("-I") || k.starts_with("-std") || k.starts_with("-O") || k.starts_with("-f"))
                    .collect();
                let status = Command::new(tc.clang(cxx)).args(tc.compile_flags()).args(flags).arg("-c").arg(a).arg("-o").arg(&obj).status()?;
                if !status.success() {
                    std::process::exit(status.code().unwrap_or(1));
                }
                objects.push(obj.into());
            },
            _ if a.starts_with("-l") => {
                let name = &a[2..];
                match lib_dirs.iter().map(|d| d.join(format!("lib{name}.a"))).find(|p| p.is_file()) {
                    Some(found) if is_wasm_archive(&found) => archives.push(found),
                    // A host library on the link line (e.g. erl_interface's libei): not linkable.
                    Some(_) => eprintln!("ampoule cc: skipping host library -l{name}"),
                    None => libs.push(name.to_string()),
                }
            },
            _ => {},
        }
    }
    if objects.is_empty() && archives.is_empty() {
        bail!("ampoule cc: nothing to archive for {out}");
    }
    tc.archive(Path::new(&out), &archives, &objects)?;
    // What the final engine link needs besides the archive: C++ runtime, extra libraries.
    let cxx = cxx || libs.iter().any(|l| matches!(l.as_str(), "stdc++" | "c++"));
    let meta = serde_json::json!({ "cxx": cxx, "libs": libs });
    std::fs::write(format!("{out}.ampoule.json"), meta.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_only_drops_linker_and_cpu_flags() {
        assert!(["-shared", "-Wl,-rpath,/x", "-march=native", "-mmacosx-version-min=11.0"].iter().all(|f| host_only(f)));
    }

    #[test]
    fn host_only_keeps_compile_flags() {
        assert!(!["-O2", "-I/x", "-DFOO=1", "-fPIC"].iter().any(|f| host_only(f)));
    }
}
