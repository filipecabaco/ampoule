# Changelog

All notable changes to ampoule. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.3] - 2026-10-15

### Added

- `--idle-stop TIME` (`AMPOULE_IDLE_STOP`, `idle_stop` in `ampoule.toml`): `start` stops the VM after
  `TIME` without connections and starts it again on the next one, holding the app's listening
  sockets so no connection is refused. An idle Realtime drops from about 150-250 MB to 2 MB; the
  next request wakes it in 0.3-0.8 s.

## [0.1.2] - 2026-10-15

### Fixed

- Every packed app kept a heap copy of its whole binary (about 33 MB for Realtime) for its lifetime.
  The runtime now maps the binary read-only: only the parts read stay resident, as clean file-backed
  pages the OS can reclaim and share between instances. Realtime's resident memory drops by about
  20 MB on Linux and macOS, with no change in boot time.

## [0.1.1] - 2026-10-14

### Fixed

- VM flags could only come from the release's `vm.args`, and scheduler counts relative to the
  processor count (`+S :4`, `+S -2`) failed or meant one processor, which is all the VM sees inside
  wasm. Flags can now also be set with `vm_args` in `ampoule.toml` and with `--vm-args` /
  `AMPOULE_VM_ARGS` at run time, and relative scheduler counts resolve against the host's cores. A
  resolved online count is capped at the total, so `+S :6` on a four-core host runs 4:4.
- Memory grew without bound while the app was idle (about 19 MB a minute for Realtime): wasi-libc's
  emulated `munmap` frees only an exact `mmap` range, so carriers ERTS released through `erl_mseg`
  stayed in linear memory. Allocators now take their carriers from `malloc`
  (`-M<A>mmmbc 0 -M<A>mmsbc 0`), which reuses them; Realtime also starts about 100 MB smaller.
- The release's `vm.args` was ignored: the runtime starts the VM without `erlexec`, so nothing read
  `-args_file`. Its `+` emulator flags, `-env` variables and `init` arguments now apply, with
  `--schedulers` still taking precedence; distribution flags are left out with a warning.

## [0.1.0] - 2026-10-13

First release: pack an Erlang/Elixir release into one binary per platform, with NIFs rebuilt for
wasm at pack time.

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
