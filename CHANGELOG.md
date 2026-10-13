# Changelog

All notable changes to ampoule. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- Renamed from beam-wasm to ampoule: the binary, `ampoule.toml`, the `AMPOULE_*` environment variables,
  the `ampoule-cc`/`ampoule-c++` NIF wrappers and the `_build/<env>/ampoule` output directory.

### Added

- SIGTERM and SIGINT stop the packed app cleanly through `init:stop()`, as with a native release
  (`--stop-timeout` / `AMPOULE_STOP_TIMEOUT`, default 30s; a second signal exits at once). SIGQUIT
  halts.
- `scripts/engine.exs`: one reproducible engine build from pinned, checksummed wasi-sdk 34, OTP
  28.5.0.4 and OpenSSL 3.5.9.
- Examples with end-to-end suites: Phoenix + Ecto, Francis, Rustler.
- `scripts/smoke.exs`: pack a fixture app with a C NIF and check the binary end to end.
- Unit tests for the runtime, CI on Linux and macOS, and a release workflow for four targets.

### Security

- Symlinks in host-backed mounts could reach host files outside the sandbox (a link to `/` in
  `/tmp`, say). Host paths are now resolved one component at a time with `openat(O_NOFOLLOW)`, and
  symlinks are resolved inside the guest's namespace.
- The guest could unlink, rename or replace a mount point itself, `/dev/null` included.
- `--deny-connect` and `--rewrite-connect` could be bypassed with IPv4-mapped IPv6 addresses, an
  IPv6 flow label or scope, or an unspecified address; a rewrite could also lead to a denied address.
- Read-only mounts did not refuse `fd_filestat_set_times`.

### Fixed

- A closed guest fd's host fd number could be reused while another thread was still using it,
  sending data to the wrong socket or file.
- A panic in a host function left the VM running without one of its threads; it now ends the
  process.
- Guest-supplied lengths and counts could panic or allocate without bound (`random_get`, iovecs,
  `poll_oneoff`), and a truncated or corrupt payload panicked instead of failing.
- Startup panicked when an environment variable or argument was not UTF-8.
- `fd_renumber` onto itself failed, `accept` and `pipe` leaked fds on errors, `SO_TYPE` and
  `fd_fdstat_get` reported host values, many host errors were reported as EIO, and symlinked
  directories inside the embedded release were not followed.
- Directories opened through a symlink listed the link's path instead of its target.
- Binding `::` with an IPv4 `--listen-host` failed.
- Two runtimes with the same pid sharing a `/tmp` (different PID namespaces) could share or delete
  each other's private directories.

- TLS client connections failed with `malformed_handshake_data`: OTP's static crypto build left
  `CRYPTO_memcmp` out.
- Each run left its private `/tmp` behind in `$TMPDIR`. It is now removed on exit (kept if it holds
  a crash dump), and directories left by killed runs are removed on the next start.
- `<binary> info | head` panicked on the closed pipe.

## [0.1.0]

- First version: pack an Erlang/Elixir release into one binary per platform, with NIFs rebuilt for
  wasm at pack time.
