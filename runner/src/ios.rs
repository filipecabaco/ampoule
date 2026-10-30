//! `ampoule pack --target ios`: a release for an iPhone or iPad app that links ERTS in.
//!
//! A device forbids starting a child process and mapping executable memory, so neither a packed
//! binary nor the wasm engine can run there. ERTS's own interpreter can, linked into the app as
//! ordinary signed code (liberlang.xcframework, built by scripts/ios.exs) and started with
//! `erl_start(argc, argv)` on a thread of the app's. This writes what the app bundles:
//!
//! ```text
//! <out>/<name>-ios/
//!   release/                the release, pruned and stripped as for a binary: no ERTS, no host native code
//!   liberlang.xcframework/  ERTS, OTP's static NIFs (crypto, asn1) and OpenSSL, for devices and the simulator
//!   runtime.json            what the runtime was built from (OTP, ERTS, OpenSSL, minimum iOS)
//! ```

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::ReleaseInfo;

pub const TARGET: &str = "ios";

/// The runtime: `$AMPOULE_IOS_RUNTIME`, else `ios/` next to this executable (the macOS releases
/// carry it), which is the directory scripts/ios.exs writes as build/ios.
pub fn locate() -> Result<PathBuf> {
    let candidates = [std::env::var_os("AMPOULE_IOS_RUNTIME").map(PathBuf::from), std::env::current_exe()?.parent().map(|d| d.join("ios"))];
    candidates.into_iter().flatten().find(|d| d.join("liberlang.xcframework").is_dir() && d.join("runtime.json").is_file()).context(
        "iOS runtime not found: put the ios/ directory from an ampoule release for macOS next to the binary, or set AMPOULE_IOS_RUNTIME (scripts/ios.exs builds it into build/ios)",
    )
}

pub fn pack(release: &Path, info: &ReleaseInfo, allow_native: bool, out_dir: &Path, runtime: &Path) -> Result<PathBuf> {
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(runtime.join("runtime.json"))?).context("runtime.json")?;
    let erts = manifest["erts_vsn"].as_str().context("runtime.json has no erts_vsn")?;
    crate::pack::check_erts(info, erts)?;

    let out = out_dir.join(format!("{}-{TARGET}", info.name));
    if out.exists() {
        std::fs::remove_dir_all(&out).with_context(|| format!("removing {}", out.display()))?;
    }
    let stats = crate::pack::write_tree(release, &out.join("release"))?;
    // ERTS is linked into the app: the release's own erts-* directory (left empty) goes too.
    for entry in std::fs::read_dir(out.join("release"))? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("erts-") {
            std::fs::remove_dir_all(entry.path())?;
        }
    }

    if !stats.foreign_native.is_empty() {
        let list = stats.foreign_native.iter().map(String::as_str).collect::<Vec<_>>().join(", ");
        if !allow_native {
            let _ = std::fs::remove_dir_all(&out);
            bail!(
                "the release has native code ({list}) an iOS app cannot load: only OTP's own (crypto, asn1) is linked into the runtime; pass --allow-native to leave it out"
            );
        }
        eprintln!("==> dropping native code (--allow-native): {list}");
    }
    for (path, program) in &stats.programs {
        eprintln!("==> warning: {} is {}; an iOS app cannot start programs", path.trim_start_matches('/'), program.describe());
    }

    copy_dir(&runtime.join("liberlang.xcframework"), &out.join("liberlang.xcframework"))?;
    std::fs::copy(runtime.join("runtime.json"), out.join("runtime.json"))?;
    eprintln!("==> {} {} for iOS: {} files, {:.1} MiB, on ERTS {erts}", info.name, info.vsn, stats.files, stats.bytes as f64 / 1_048_576.0);
    Ok(out)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest).with_context(|| format!("copying {}", entry.path().display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(dir: &Path, erts: &str) -> PathBuf {
        let rt = dir.join("runtime");
        std::fs::create_dir_all(rt.join("liberlang.xcframework/ios-arm64")).unwrap();
        std::fs::write(rt.join("liberlang.xcframework/ios-arm64/liberlang.a"), b"!<arch>\n").unwrap();
        std::fs::write(rt.join("runtime.json"), format!(r#"{{"format":1,"erts_vsn":"{erts}"}}"#)).unwrap();
        rt
    }

    fn release(dir: &Path) -> PathBuf {
        let rel = dir.join("rel");
        for d in ["bin", "erts-16.4/bin", "lib/app-1/ebin", "lib/crypto-5.6/priv/lib", "releases/1"] {
            std::fs::create_dir_all(rel.join(d)).unwrap();
        }
        std::fs::write(rel.join("bin/app"), b"#!/bin/sh\n").unwrap();
        std::fs::write(rel.join("erts-16.4/bin/beam.smp"), b"\xcf\xfa\xed\xfe").unwrap();
        std::fs::write(rel.join("lib/app-1/ebin/app.app"), b"{application, app, []}.").unwrap();
        std::fs::write(rel.join("lib/crypto-5.6/priv/lib/crypto.so"), b"\xcf\xfa\xed\xfe").unwrap();
        std::fs::write(rel.join("releases/1/start.boot"), b"boot").unwrap();
        rel
    }

    fn info() -> ReleaseInfo {
        ReleaseInfo { name: "app".into(), vsn: "1".into(), erts_vsn: "16.4".into(), elixir: false }
    }

    #[test]
    fn stages_the_release_without_erts_or_host_native_code_beside_the_runtime() {
        let dir = crate::TestDir::new("ios-pack");
        let out = pack(&release(&dir.0), &info(), false, &dir.0.join("out"), &runtime(&dir.0, "16.4.0.4")).unwrap();
        let rel = out.join("release");
        assert!(rel.join("lib/app-1/ebin/app.app").is_file() && rel.join("releases/1/start.boot").is_file());
        assert!(!rel.join("bin").exists() && !rel.join("erts-16.4").exists());
        assert!(!rel.join("lib/crypto-5.6/priv/lib/crypto.so").exists());
        assert!(out.join("liberlang.xcframework/ios-arm64/liberlang.a").is_file() && out.join("runtime.json").is_file());
    }

    #[test]
    fn refuses_native_code_outside_otp_unless_allowed() {
        let dir = crate::TestDir::new("ios-native");
        let rel = release(&dir.0);
        std::fs::create_dir_all(rel.join("lib/sqlite-1/priv")).unwrap();
        std::fs::write(rel.join("lib/sqlite-1/priv/sqlite3_nif.so"), b"\xcf\xfa\xed\xfe").unwrap();
        let rt = runtime(&dir.0, "16.4.0.4");
        let out = dir.0.join("out");

        let refused = pack(&rel, &info(), false, &out, &rt).unwrap_err().to_string();
        assert!(refused.contains("sqlite3_nif.so") && !out.join("app-ios").exists(), "{refused}");
        assert!(pack(&rel, &info(), true, &out, &rt).is_ok());
    }

    #[test]
    fn refuses_a_runtime_of_another_otp() {
        let dir = crate::TestDir::new("ios-erts");
        let err = pack(&release(&dir.0), &info(), false, &dir.0.join("out"), &runtime(&dir.0, "15.2.7")).unwrap_err();
        assert!(err.to_string().contains("ERTS 15.2.7"), "{err}");
    }
}
