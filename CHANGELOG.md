# Changelog

All notable changes to ampoule. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.8] - 2026-10-22

### Changed

- Packed binaries are about 12% smaller (examples/francis: 13.4 MiB in 0.1.7, 11.8 MiB now; c_nif
  17.0 → 15.1 MiB), with the same startup and throughput:
  - The release image is compressed with a zstd dictionary trained on the release's own files and
    stored in the image, so its many small `.beam` files share what they have in common; every file
    is still its own frame, read when the app opens it (francis: 5.2 → 4.6 MiB).
  - The precompiled engine is compressed at zstd level 19 instead of 9 (0.6 MB smaller; decompressing
    takes the same time). Packing an app with NIFs takes about 3 s longer per target.
  - The lean runtime leaves out wasmtime's `addr2line` and `demangle`, which only make a native
    backtrace of the runtime itself readable (0.4 MB). The full `ampoule` keeps them.
- The release image format is `RTFS0002` (the dictionary); images of the earlier format still read.

## [0.1.7] - 2026-10-21

### Added

- Programs the release ships in its own `priv/` directories run, by default without
  `[process] allow`: `System.cmd(Path.join(:code.priv_dir(:my_app), "bin/tool"), args)` and
  `{:spawn_executable, ...}` ports work as on a native release. The first start extracts the app's
  whole `priv/` to the private `/tmp` (`.ampoule/priv/<app>-<vsn>/`, same layout and modes, once per
  run) and runs the program on the host from there. `[process] priv = false` turns it off.
- Packing keeps `.sh` scripts under `priv/`, and warns when an executable in an app's `priv/` is
  built for another OS or CPU than the target (ELF and Mach-O, universal binaries included).
- Erlang distribution. `--node NAME` (`RELEASE_NODE`, or `[distribution]` in `ampoule.toml`) starts
  the app as a distributed node that clusters with other packed nodes and with native ones. By
  default no epmd is needed: every node listens on one distribution port (9100) and reaches its
  peers there, OTP's `-erl_epmd_port`; `epmd = "host"` registers with an epmd running on the host
  instead. The cookie comes from `--cookie-file`, `RELEASE_COOKIE`, `cookie_file` or the release's
  own, and is never printed. The distribution port is allowed by `--listen` and mapped by
  `--port-map`; peers are reached through `--rewrite-connect` and `--deny-connect`. See
  `examples/cluster`.
- `remote` and `rpc EXPR` attach a shell to a running node, or evaluate an expression there, as a
  release's `bin/<name> remote` and `rpc` do.

### Changed

- Packed binaries are about 11 MiB smaller (examples/francis: 24.6 MiB in 0.1.6, 13.4 MiB now).
  They start from the lean runtime, `ampoule` built without the Cranelift compiler, which a packed
  app never uses (8.4 MiB): releases ship it next to `ampoule` in the archive and as the bare
  `ampoule-<target>`, and packing uses it when it sits next to `ampoule`, refusing one from another
  release. The runtime is also stripped of symbols (2.8 MiB).
- The app sees `/tmp/.ampoule`, the runtime's own files (the sys.config overlay, the source of
  `/etc/hosts`, extracted `priv/` programs), read-only.
- `vm.args` flags `-name`, `-sname` and `-setcookie` are still left out with a warning, which no
  longer shows the cookie; `-hidden`, `-proto_dist`, `-epmd_module` and `-dist_listen` apply when
  the node is distributed, and `-erl_epmd_port` sets its port.

### Fixed

- Memory the VM freed never went back to the host: after a spike the process's RSS stayed at its
  peak until the VM stopped, since wasm memory cannot shrink and wasi-libc's `malloc` keeps what it
  frees. ERTS's carriers now come from an allocator of whole wasm pages in the engine
  (`engine/ampoule-os`), and the pages of a carrier ERTS releases are dropped on the host
  (`madvise(MADV_DONTNEED)` on Linux, a fresh mapping on macOS) and reused for later carriers. After
  holding and freeing 1 GiB of binaries, RSS goes back to about 115 MiB (idle: 94 MiB), where it
  stayed at about 1.1 GiB, three spikes in a row (Apple M5 Pro). Carriers come from `erl_mseg`
  again, so the `-M<A>mmmbc 0 -M<A>mmsbc 0` defaults from 0.1.1 are gone; the VM's address space
  still stays at the size a spike reached, counted against `--memory-max`.

## [0.1.6] - 2026-10-20

### Added

- NIFs in the project's own C or C++ code (`c_src/` with `elixir_make`, or any Mix compiler that
  runs `$(CC)`) are rebuilt for wasm when packing, as the project's own Rust NIFs in `native/`
  already were. Before, packing failed with "neither a dependency nor has native/".
- The engine relinked with an app's NIFs is cached in `~/.cache/ampoule/engines/`, keyed by a
  hash of everything the link reads, so packing again with the same NIFs skips the link and gives
  the same binary (examples/c_nif: 13.1 s → 1.3 s; rustler 16.9 s → 4.6 s; sqlite 22.2 s → 10.6 s,
  the rest being SQLite's build). `AMPOULE_NO_CACHE=1` turns it off.
- Packing for another target finds that target's runtime (`ampoule-<target>` from a release) next
  to the `ampoule` binary when neither `--stub` nor `AMPOULE_STUBS` names one, so
  `ampoule release --target x86_64-linux` works after downloading it there.
- Releases also publish each archive as `ampoule-<target>.tar.gz`, unpacking to `ampoule/`, so
  `releases/latest/download/ampoule-<target>.tar.gz` always installs the newest version with its
  NIF kit.

### Fixed

- NIF dependencies built by `elixir_make` with precompiled binaries (`cc_precompiler`, as exqlite
  uses) are built from source when packing; they used to download a library built for the host
  and fail with "no wasm archive".
- SQLite's amalgamation builds under WASI inside a NIF: memory-mapped I/O is off (it did not
  compile with `__linux__` defined), and loading extensions, which WASI leaves out, returns an
  error instead of failing to link. exqlite now packs; see `examples/sqlite`.

## [0.1.5] - 2026-10-20

### Changed

- Faster. Measured on examples/francis (Apple M5 Pro, against v0.1.4): HTTP throughput +30% with
  25% less CPU per request, and interpreter-bound Erlang 12–14% faster.
  - Bounds checks on the guest's memory no longer take wasmtime's memory lock, which every
    scheduler thread contended for on every system call.
  - The engine is built with link-time optimisation (`-O3`, `--lto-O3`); `beam.wasm` is 9.7%
    larger.
  - The macOS arm64 engine is compiled for LSE, the single-instruction atomics every Apple Silicon
    CPU has. Linux arm64 keeps the ARMv8.0 baseline, so ARMv8.0 cores still run it.
- Packing an app with NIFs needs wasi-sdk 34 exactly: the engine kit is now LLVM bitcode, and
  `ampoule` stops with a clear message when the wasi-sdk's LLVM differs. Relinking takes about 12 s
  instead of 1 s.

## [0.1.4] - 2026-10-20

### Added

- Host programs: ports, `System.cmd/3` and `:os.cmd/1` start the host programs listed in
  `[process] allow` in `ampoule.toml` (or `--allow-exec`, `AMPOULE_ALLOW_EXEC`), on macOS and
  Linux. Names are looked up on `PATH`, and `*` allows everything on it. Each allowed program is
  mounted read-only at its host path, so `System.find_executable/1` finds it. Nothing is allowed by
  default, and a program outside the list fails with `:eacces`.
- Unix domain sockets: `{:local, path}` with `gen_tcp` and `gen_udp`, at the app's own paths. A
  socket the app binds lands in a directory it may write; it can connect to sockets its mounts show,
  host services included. Abstract names are refused.
- The memory alarm: `start` sets OTP's `{:system_memory_high_watermark, []}` through
  `:alarm_handler` at 80% of the VM's memory limit and clears it 10 points lower (`memory_alarm`,
  `--memory-alarm`, `AMPOULE_MEMORY_ALARM`). `/proc/meminfo` reports the limit as `MemTotal`.
- `restart = "on-failure"` (`--restart`, `AMPOULE_RESTART`): `start` starts a VM that exits non-zero,
  out of memory included, again after a backoff, keeping its listeners open in between; after 5
  failures in 5 minutes it exits with the VM's status.

- `File.chmod/2` (and `:file.write_file_info/2`'s mode) sets the mode of the host file behind a
  writable path; it failed with `:enosys`. setuid and setgid bits are dropped, so a guest running
  as root cannot leave a setuid-root program where the host can run it.

### Fixed

- `System.pid/0` (`os:getpid/0`) is the host pid of the process running the VM, which `ps` lists and
  `kill` reaches; it was always `"42"`, wasi-libc's stand-in.
- `File.stat/1` reports permission bits (`mode`) for files the app can see; it reported only the
  file type.
- `dup/1` works for NIFs; wasi-libc's returned `ENOTSUP`.
- `File.mkdir/1` on an existing mount point, `/app` or a directory joining mounts returns `:eexist`
  instead of `:erofs` or `:ebusy`, so `File.mkdir_p/1` works for paths that already exist there.

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
