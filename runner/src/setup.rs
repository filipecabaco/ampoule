//! `ampoule setup`: install what packing an app with NIFs needs, the wasi-sdk and the engine kit,
//! into the user cache (`~/.cache/ampoule`, or under `$XDG_CACHE_HOME`), where `Toolchain::locate`
//! looks. Any project (a sibling checkout, CI, a fresh machine) can then pack native code without
//! setting `AMPOULE_WASI_SDK` and `AMPOULE_KIT`. `locate` runs the same install on its own when
//! either is missing, unless `AMPOULE_NO_FETCH` is set; `ampoule setup` is the explicit form, for
//! warming a cache ahead of time.
//!
//! The wasi-sdk is the pinned release from GitHub, SHA-256 checked, kept in `wasi-sdk-<version>/`.
//! The kit comes out of this ampoule's own release archive (`v<version>`, checked against its
//! `SHA256SUMS`), kept in `kit-<erts>/`. Fetching and unpacking shell out to `curl` and `tar`, as
//! `fetch.rs` does, so the runner needs no HTTP stack.
//!
//! The wasi-sdk version and checksums are pinned in scripts/engine.exs too; a test keeps the two
//! in step.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

pub const WASI_SDK: &str = "34.0";

/// Directory names under the cache; versioned so a new pin installs beside the old one.
pub fn sdk_name() -> String {
    format!("wasi-sdk-{WASI_SDK}")
}

pub fn kit_name() -> String {
    format!("kit-{}", crate::ERTS_VSN)
}

/// The wasi-sdk release's name for a host, and the SHA-256 of its tarball.
fn host_pin(arch: &str, os: &str) -> Result<(&'static str, &'static str)> {
    Ok(match (arch, os) {
        ("aarch64", "macos") => ("arm64-macos", "9c59398106b417f8f14913380fdf0097a8cc0ff4af9eb3ce0065a859e88d49e9"),
        ("x86_64", "macos") => ("x86_64-macos", "87d27fa8adc68dee59bfbf2e22a6d34ef717c34d6bf1d8af2a56fc929d9ce0eb"),
        ("aarch64", "linux") => ("arm64-linux", "f7e243dff54d60bcc576e94d6166b69f410f2500ae4a9ceef34315be10e77971"),
        ("x86_64", "linux") => ("x86_64-linux", "b761e3a0721dbae9c09a0059e5fdb2bf917d1b4a8a7b430fb3b5aafb0984b2c4"),
        (a, o) => bail!("there is no wasi-sdk build for {a}-{o}; download one from https://github.com/WebAssembly/wasi-sdk/releases and set AMPOULE_WASI_SDK"),
    })
}

pub fn setup_main(args: &[String]) -> Result<()> {
    let force = args.iter().any(|a| a == "--force");
    if let Some(bad) = args.iter().find(|a| *a != "--force") {
        bail!("unknown option {bad} (usage: ampoule setup [--force])");
    }
    install(true, true, force)?;
    for (var, what) in [("AMPOULE_WASI_SDK", "wasi-sdk"), ("AMPOULE_KIT", "kit")] {
        if let Some(p) = std::env::var_os(var) {
            eprintln!("note: {var} is set ({}) and is used instead of the cached {what}", PathBuf::from(p).display());
        }
    }
    Ok(())
}

/// Install the wasi-sdk and/or the kit into the cache; an existing install is kept unless `force`.
pub fn install(sdk: bool, kit: bool, force: bool) -> Result<()> {
    let cache = crate::toolchain::cache_dir().context("there is no HOME to keep the cache in; set AMPOULE_WASI_SDK and AMPOULE_KIT instead")?;
    std::fs::create_dir_all(&cache).with_context(|| format!("creating {}", cache.display()))?;
    if sdk {
        install_sdk(&cache, force)?;
    }
    if kit {
        install_kit(&cache, force)?;
    }
    Ok(())
}

fn install_sdk(cache: &Path, force: bool) -> Result<()> {
    let dest = cache.join(sdk_name());
    if dest.join("bin/clang").exists() && !force {
        eprintln!("wasi-sdk {WASI_SDK} is already installed at {} (--force reinstalls)", dest.display());
        return Ok(());
    }
    let (host, want) = host_pin(std::env::consts::ARCH, std::env::consts::OS)?;
    let major = WASI_SDK.split('.').next().unwrap_or(WASI_SDK);
    let url = format!("https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-{major}/wasi-sdk-{WASI_SDK}-{host}.tar.gz");
    eprintln!("==> downloading wasi-sdk {WASI_SDK}, needed to build NIFs: {url}");
    unpack_download(cache, &url, want, &dest, "wasi-sdk")
}

fn install_kit(cache: &Path, force: bool) -> Result<()> {
    let dest = cache.join(kit_name());
    if dest.join("include").is_dir() && !force {
        eprintln!("the engine kit is already installed at {} (--force reinstalls)", dest.display());
        return Ok(());
    }
    if !crate::fetch::enabled() {
        bail!("the engine kit is not installed and AMPOULE_NO_FETCH is set; set AMPOULE_KIT to a kit/ directory");
    }
    let version = crate::VERSION;
    let name = format!("ampoule-{version}-{}.tar.gz", crate::TARGET);
    let base = crate::fetch::release_base(version);
    eprintln!("==> downloading the engine kit from ampoule {version} ({base}/{name})");
    let sums = crate::fetch::curl(&format!("{base}/SHA256SUMS"))?;
    let want = crate::fetch::expected(&String::from_utf8_lossy(&sums), &name)
        .with_context(|| format!("{name} is not in the SHA256SUMS of ampoule {version}; a development build has no release to take the kit from, so set AMPOULE_KIT (scripts/engine.exs builds it into build/engine/kit)"))?;
    // The archive unpacks to `ampoule-<version>-<target>/kit`: the kit is that one subdirectory.
    unpack_download_with(cache, &format!("{base}/{name}"), &want, &dest, "kit", 1, Some("kit"))
}

fn unpack_download(cache: &Path, url: &str, sha256: &str, dest: &Path, tag: &str) -> Result<()> {
    unpack_download_with(cache, url, sha256, dest, tag, 1, None)
}

/// Download `url` (checked against `sha256`), unpack it under a private staging name with
/// `strip` leading components, and swap `sub` of it (or all of it) into `dest`. The old `dest`
/// is moved aside first and deleted only once the new one is in place.
fn unpack_download_with(cache: &Path, url: &str, sha256: &str, dest: &Path, tag: &str, strip: u32, sub: Option<&str>) -> Result<()> {
    let pid = std::process::id();
    let tarball = cache.join(format!(".{tag}.tar.gz.partial-{pid}"));
    let staging = cache.join(format!(".{tag}.partial-{pid}"));
    let cleanup = || {
        let _ = std::fs::remove_file(&tarball);
        let _ = std::fs::remove_dir_all(&staging);
    };
    let result = (|| -> Result<()> {
        let status = Command::new("curl").args(["-fL", "--retry", "3", "-o"]).arg(&tarball).arg(url).status().context("downloading needs curl on PATH")?;
        if !status.success() {
            bail!("downloading {url} failed");
        }
        let got = sha256_file(&tarball)?;
        if got != sha256 {
            bail!("{url}: SHA-256 {got}, expected {sha256}");
        }
        std::fs::create_dir_all(&staging)?;
        let status =
            Command::new("tar").arg("-xzf").arg(&tarball).arg(format!("--strip-components={strip}")).arg("-C").arg(&staging).status().context("running tar")?;
        if !status.success() {
            bail!("unpacking {url} failed");
        }
        let src = match sub {
            Some(s) => staging.join(s),
            None => staging.clone(),
        };
        if !src.is_dir() {
            bail!("{url} has no {} inside", sub.unwrap_or("content"));
        }
        let old = cache.join(format!(".{tag}.old-{pid}"));
        let had_old = dest.exists();
        if had_old {
            std::fs::rename(dest, &old).with_context(|| format!("moving {} aside", dest.display()))?;
        }
        if let Err(e) = std::fs::rename(&src, dest) {
            if had_old {
                let _ = std::fs::rename(&old, dest);
            }
            return Err(e).with_context(|| format!("moving {tag} to {}", dest.display()));
        }
        let _ = std::fs::remove_dir_all(&old);
        Ok(())
    })();
    cleanup();
    result?;
    eprintln!("==> {tag} installed at {}", dest.display());
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(crate::fetch::hex(&h.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// scripts/engine.exs pins the same wasi-sdk; a bump in one must be a bump in both.
    #[test]
    fn pins_match_engine_script() {
        let engine = include_str!("../../scripts/engine.exs");
        assert!(engine.contains(&format!("@wasi_sdk \"{WASI_SDK}\"")), "WASI_SDK differs from scripts/engine.exs");
        for (arch, os) in [("aarch64", "macos"), ("x86_64", "macos"), ("aarch64", "linux"), ("x86_64", "linux")] {
            let (host, sha) = host_pin(arch, os).unwrap();
            assert!(engine.contains(&format!("\"{host}\" => \"{sha}\"")), "{host} differs from scripts/engine.exs");
        }
    }

    #[test]
    fn unsupported_hosts_say_what_to_do() {
        assert!(host_pin("riscv64", "linux").unwrap_err().to_string().contains("AMPOULE_WASI_SDK"));
    }

    #[test]
    fn cache_names_are_versioned() {
        assert_eq!(sdk_name(), format!("wasi-sdk-{WASI_SDK}"));
        assert!(kit_name().starts_with("kit-"));
    }
}
