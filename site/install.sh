#!/bin/sh
# Installs ampoule: the archive for this machine, then the lean runtime of every other platform,
# so `ampoule release --target T` packs for any of them, offline once installed. Every file is
# checked against the SHA256SUMS of the same release. Nothing outside the install directory is
# touched: no sudo, no shell profile edits; it prints the PATH line for you to add.
#
#   curl -fsSL https://filipecabaco.github.io/ampoule/install.sh | sh
#   curl -fsSL https://filipecabaco.github.io/ampoule/install.sh | AMPOULE_VERSION=0.1.12 sh
#
# To uninstall, remove the install directory and the PATH line.
#
# Settings, as environment variables:
#   AMPOULE_HOME=DIR          where to install (default: ~/ampoule)
#   AMPOULE_VERSION=0.1.12    a release to install (default: the latest)
#   AMPOULE_ALL_RUNTIMES=0    only this machine's runtime, not the other platforms'

set -eu

repo="https://github.com/filipecabaco/ampoule/releases"
home_dir="${AMPOULE_HOME:-$HOME/ampoule}"
targets="aarch64-darwin x86_64-darwin aarch64-linux x86_64-linux"

say() { printf '%s\n' "$*"; }
fail() { printf 'ampoule install: %s\n' "$*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || fail "needs curl"
command -v tar >/dev/null 2>&1 || fail "needs tar"

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  fail "needs sha256sum or shasum to check the download"
fi

# This machine's target. A shell running under Rosetta reports x86_64 on Apple silicon; the arm64
# build is the one to install there.
case "$(uname -s)" in
  Darwin) os=darwin ;;
  Linux) os=linux ;;
  *) fail "no build for $(uname -s): ampoule runs on macOS and Linux" ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=aarch64 ;;
  x86_64 | amd64) arch=x86_64 ;;
  *) fail "no build for $(uname -m): ampoule runs on arm64 and x86_64" ;;
esac
if [ "$os" = darwin ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
  arch=aarch64
fi
host="$arch-$os"

# The latest release, resolved once: every download then comes from that one release, so a
# release published mid-install cannot pair one version's archive with another's SHA256SUMS.
if [ -n "${AMPOULE_VERSION:-}" ]; then
  version="${AMPOULE_VERSION#v}"
else
  latest="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "$repo/latest")" || fail "could not find the latest release"
  version="${latest##*/v}"
  [ -n "$version" ] && [ "$version" != "$latest" ] || fail "could not find the latest release in $latest"
fi
base="$repo/download/v$version"
archive="ampoule-$version-$host.tar.gz"
top="ampoule-$version-$host"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

fetch() { curl -fsSL --retry 3 -o "$tmp/$1" "$base/$1" || fail "could not download $base/$1"; }

check() {
  want="$(awk -v f="$1" '{ n = $2; sub(/^\*/, "", n); if (n == f) print $1 }' "$tmp/SHA256SUMS")"
  [ -n "$want" ] || fail "$1 is not in the release's SHA256SUMS"
  [ "$(sha256 "$tmp/$1")" = "$want" ] || fail "$1 does not match its SHA256SUMS entry"
}

say "Installing ampoule $version for $host into $home_dir"
fetch SHA256SUMS
fetch "$archive"
check "$archive"
tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$home_dir"
# An upgrade replaces what an earlier install put here, and only that, so no stale file stays.
for old in ampoule ampoule-aarch64-darwin ampoule-x86_64-darwin ampoule-aarch64-linux ampoule-x86_64-linux kit ios docs LICENSE NOTICE README.md; do
  rm -rf "${home_dir:?}/$old"
done
cp -R "$tmp/$top/." "$home_dir/"

if [ "${AMPOULE_ALL_RUNTIMES:-1}" != 0 ]; then
  for target in $targets; do
    [ "$target" = "$host" ] && continue
    say "Adding the $target runtime, to pack for it"
    fetch "ampoule-$target"
    check "ampoule-$target"
    cp "$tmp/ampoule-$target" "$home_dir/ampoule-$target"
    chmod +x "$home_dir/ampoule-$target"
  done
fi

say ""
"$home_dir/ampoule" version
case ":$PATH:" in
  *":$home_dir:"*) say "Run \`ampoule release\` in a Mix project to pack it." ;;
  *)
    say "Add ampoule to your PATH, in your shell's profile too:"
    say "  export PATH=\"$home_dir:\$PATH\""
    ;;
esac
