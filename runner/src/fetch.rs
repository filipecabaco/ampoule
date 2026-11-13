//! The lean runtime of another target, downloaded from this ampoule's own release when packing
//! asks for a target whose `ampoule-<target>` is not next to `ampoule`.
//!
//! The file comes from `v<version>` (never `latest`), is checked against that release's
//! `SHA256SUMS`, and is kept in `~/.cache/ampoule/runtimes/<version>/` for the next pack. `curl`
//! does the download, so the binary carries no HTTP or TLS client. `AMPOULE_NO_FETCH=1` turns this
//! off; `AMPOULE_RELEASES` points at a mirror holding the same files.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

const RELEASES: &str = "https://github.com/filipecabaco/ampoule/releases/download";

/// Whether a missing runtime may be downloaded (`AMPOULE_NO_FETCH` unset, empty or `0`).
pub fn enabled() -> bool {
    std::env::var_os("AMPOULE_NO_FETCH").is_none_or(|v| v.is_empty() || v == "0")
}

/// Where `v<version>`'s files are downloaded from (`AMPOULE_RELEASES` points at a mirror).
pub fn release_base(version: &str) -> String {
    std::env::var("AMPOULE_RELEASES").map(|b| format!("{}/v{version}", b.trim_end_matches('/'))).unwrap_or_else(|_| format!("{RELEASES}/v{version}"))
}

/// Where this version's downloaded runtimes are kept.
fn dir(version: &str) -> Option<PathBuf> {
    Some(crate::toolchain::cache_dir()?.join("runtimes").join(version))
}

/// `ampoule-<target>` for `version`: the cached copy, else downloaded and checked.
pub fn runtime(target: &str, version: &str) -> Result<PathBuf> {
    let name = format!("ampoule-{target}");
    let dir = dir(version).context("no HOME to keep the downloaded runtime in")?;
    let path = dir.join(&name);
    if path.is_file() {
        return Ok(path);
    }
    let base = release_base(version);
    eprintln!("==> downloading the {target} runtime of ampoule {version} into {}", dir.display());
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let partial = dir.join(format!(".{name}.partial"));
    let sums = curl(&format!("{base}/SHA256SUMS"))?;
    let want = expected(&String::from_utf8_lossy(&sums), &name).with_context(|| format!("{name} is not in the SHA256SUMS of ampoule {version}"))?;
    let bytes = curl(&format!("{base}/{name}"))?;
    let got = hex(&Sha256::digest(&bytes));
    if got != want {
        bail!("{name} from {base} does not match its SHA256SUMS entry ({got}, expected {want})");
    }
    write_executable(&partial, &bytes)?;
    std::fs::rename(&partial, &path).with_context(|| format!("moving {} into place", path.display()))?;
    Ok(path)
}

/// The checksum `SHA256SUMS` lists for `name` (`<hex>  <name>`, or `<hex> *<name>`).
pub fn expected(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim_start().trim_start_matches('*') == name).then(|| hash.to_ascii_lowercase())
    })
}

pub fn curl(url: &str) -> Result<Vec<u8>> {
    let out = Command::new("curl").args(["-fsSL", "--retry", "3", url]).output().context("downloading the runtime needs curl on PATH")?;
    if !out.status.success() {
        bail!("could not download {url}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

fn write_executable(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).with_context(|| format!("chmod {}", path.display()))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256sums_names_each_file_once() {
        let sums = "ABC123  ampoule-x86_64-linux\nfff *ampoule-aarch64-linux\n000  ampoule-x86_64-linux.tar.gz\n";
        assert_eq!(expected(sums, "ampoule-x86_64-linux").as_deref(), Some("abc123"));
        assert_eq!(expected(sums, "ampoule-aarch64-linux").as_deref(), Some("fff"));
        assert_eq!(expected(sums, "ampoule-x86_64-darwin"), None);
    }

    #[test]
    fn runtimes_are_kept_per_version() {
        let d = dir("0.1.12").unwrap();
        assert!(d.ends_with("ampoule/runtimes/0.1.12"));
    }
}
