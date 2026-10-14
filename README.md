<p align="center"><img src="assets/logo.svg" alt="ampoule logo: the bytes 00 61 73 6D that open every WebAssembly module, as four tiles" width="128"></p>

# ampoule

ampoule turns a Mix project into one executable file. Copy the file to a Mac or a Linux machine
and run it: the machine needs no Erlang, no Elixir and no shared libraries.

The project site is at [filipecabaco.github.io/ampoule](https://filipecabaco.github.io/ampoule/).

```console
$ cd my_app
$ ampoule release
==> MIX_ENV=prod mix release --overwrite
==> my_app 0.1.0: 1018 files, 11.6 MiB → 5.2 MiB image (32 paths pruned)
_build/prod/ampoule/my_app-aarch64-darwin (26.3 MiB)
==> packed in 0.3s; run `<binary> help` for its commands

$ _build/prod/ampoule/my_app-aarch64-darwin start
[info] Running MyApp with Bandit 1.11.1 at 0.0.0.0:4000 (http)
```

That is the whole workflow. The file is your release, unchanged, plus the VM to run it, and it
behaves like `bin/my_app`: `start`, `eval`, your own commands such as `migrate`, environment
variables for configuration, and a clean `init:stop()` on SIGTERM. The numbers come from
[`examples/francis`](examples/francis), which packs to 26 MB and answers its first request about 0.3
seconds after launch.

## Contents

- [Ship it](#ship-it)
- [Bring your app as it is](#bring-your-app-as-it-is)
- [Install](#install)
- [Reference](#reference)
- [Limitations](#limitations)
- [How it works](#how-it-works)
- [Contributing](#contributing)
- [License](#license)

## Ship it

### To a server

The Linux binaries are statically linked, so any Linux machine of the right architecture runs them.

```sh
scp _build/prod/ampoule/my_app-x86_64-linux server:
ssh server 'DATABASE_URL=... PORT=8080 ./my_app-x86_64-linux serve'
```

### In an empty container

The binary is the only file the image needs:

```dockerfile
FROM scratch
COPY my_app-aarch64-linux /my_app
ENTRYPOINT ["/my_app"]
CMD ["start"]
```

`docker stop` stops the app through `init:stop()`, the same as a native release. If the app makes
TLS connections, give it CA certificates as well, for example
`COPY --from=alpine:3.22 /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/`.

### To every platform from one machine

Packing for another platform uses that platform's `ampoule` binary as the base. Each [release](https://github.com/filipecabaco/ampoule/releases)
publishes them as `ampoule-<target>`. Put them in one directory and pass the targets:

```sh
AMPOULE_STUBS=~/ampoule-stubs ampoule release --target aarch64-darwin --target x86_64-linux
```

The Linux binary for a Docker image can come from a Mac this way, NIFs included.

## Bring your app as it is

Phoenix, Ecto, Bandit, Francis and plain OTP applications pack without changes.

An app that migrates its database before starting describes that order in `ampoule.toml`, which
`ampoule release` picks up from the project root:

```toml
serve = ["migrate", "start"]

[commands]
migrate = "MyApp.Release.migrate()"

[start.env]
PHX_SERVER = "true"
```

`<binary> serve` then runs the migrations and starts the app, and `<binary> migrate` runs them on
their own.

An app with NIFs gets them rebuilt for wasm during `ampoule release`, from C, C++ or Rust (Rustler).
The output shows each one:

```console
==> building eiconv for wasm (priv/eiconv_nif.so)
==> linking the engine with 1 NIF(s): eiconv_nif
_build/prod/ampoule/my_app-aarch64-darwin (35.5 MiB)
```

This needs wasi-sdk on the packing machine (see [NIFs](#nifs)). The machine that runs the binary
needs nothing.

[`examples/`](examples) has three complete apps, each with an end-to-end suite that packs the app,
runs the binary and tests it over HTTP:

| Example | Shows |
|---|---|
| [`phoenix_ecto`](examples/phoenix_ecto) | Phoenix JSON API on Ecto and Postgres; `serve` creates and migrates the database, then starts |
| [`francis`](examples/francis) | Francis: HTML, JSON, server-sent events, the network policy |
| [`rustler`](examples/rustler) | a Rustler NIF in the project's own `native/` crate |

## Install

Download the archive for your machine from the [releases page](https://github.com/filipecabaco/ampoule/releases). There is one for
`aarch64-darwin`, `x86_64-darwin`, `aarch64-linux` and `x86_64-linux`, each holding `ampoule`, the
NIF kit (`kit/`) and the license. Keep `kit/` next to `ampoule`.

```sh
curl -fsSL https://github.com/filipecabaco/ampoule/releases/download/v0.1.0/ampoule-0.1.0-aarch64-darwin.tar.gz | tar -xz
export PATH="$PWD/ampoule-0.1.0-aarch64-darwin:$PATH"
ampoule version
```

If the app has no NIFs, the bare binary is enough:
`curl -fLo ampoule https://github.com/filipecabaco/ampoule/releases/latest/download/ampoule-aarch64-darwin && chmod +x ampoule`.

Packing needs Elixir and Erlang/OTP 28 on the packing machine, since `ampoule release` runs
`mix release`. To build ampoule itself, see [CONTRIBUTING.md](CONTRIBUTING.md).

## Reference

### Packaging

| Command | What it does |
|---|---|
| `ampoule release [pack options] [-- mix release args]` | runs `mix release --overwrite` in the current project, then packs it |
| `ampoule pack RELEASE_DIR [pack options]` | packs an existing release directory |
| `ampoule run RELEASE_DIR [--config FILE] [app options] COMMAND` | runs a release directory without packing it, for quick iteration |
| `ampoule version` | runtime version, target and ERTS version |

Pack options:

| Option | Default | |
|---|---|---|
| `--config FILE` | `./ampoule.toml` if present | see [Configuration](#configuration) |
| `--out DIR` | `_build/<env>/ampoule` | where binaries are written |
| `--target T` | the host's | `aarch64-darwin`, `x86_64-darwin`, `aarch64-linux`, `x86_64-linux`; repeatable |
| `--stub T=FILE` | `$AMPOULE_STUBS/ampoule-T` | the `ampoule` binary built for target `T` |
| `--allow-native` | off | drop native code (NIFs) instead of rebuilding it or refusing to pack |
| `--env ENV` | `$MIX_ENV`, else `prod` | `MIX_ENV` for `release` |
| `--name NAME` | the only release | which release to build or pack when the project defines several |

The release must be built with the same OTP major as the runtime's ERTS (`ampoule version` shows
it; currently OTP 28, ERTS 16). `pack` refuses a mismatch.

### Running the packed app

```
<binary> [options] <command>
```

| Command | |
|---|---|
| `start` | start the release (like `bin/my_app start`) |
| `serve` | run the `serve` sequence from the config (default: `start`) |
| `eval EXPR` | evaluate an expression on a release that is not started (like `bin/my_app eval`) |
| `vm -- ERL_ARGS` | run the VM with raw `erl` arguments; the release is at `/app` |
| `info` | what is packed: app, file count, runtime, commands, NIFs, network policy |
| `<name>` | a custom command from `[commands]` |

Each option has an environment-variable form, and a flag wins over its variable:

| Flag | Variable | |
|---|---|---|
| `--listen PORTS` | `AMPOULE_LISTEN` | ports the app may bind, e.g. `4000,4001` (default: any) |
| `--port-map G:H,...` | `AMPOULE_PORT_MAP` | bind app port `G` as host port `H` |
| `--listen-host IP` | `AMPOULE_LISTEN_HOST` | the address wildcard binds (`0.0.0.0`) land on |
| `--host NAME=IP,...` | `AMPOULE_HOSTS` | extra `/etc/hosts` entries |
| `--rewrite-connect A=B` | `AMPOULE_REWRITE` | send connections for `A` (ip:port) to `B` |
| `--deny-connect ADDR` | `AMPOULE_DENY` | refuse connections to `ADDR` (ip:port) |
| `--mount H:G[:ro],...` | `AMPOULE_MOUNTS` | expose host path `H` at guest path `G` |
| `--memory-max SIZE` | `AMPOULE_MEMORY_MAX` | cap the VM's memory, e.g. `512M`, `2G` |
| `--schedulers N` | `AMPOULE_SCHEDULERS` | scheduler threads (default: `+S` in vm.args, else host cores) |
| `--stop-timeout SECS` | `AMPOULE_STOP_TIMEOUT` | how long SIGTERM and SIGINT wait for a clean stop (default: 30) |
| `--tmp DIR` | `AMPOULE_TMP` | host directory behind `/tmp` (default: a private one) |
| | `AMPOULE_CA_BUNDLE` | CA certificates file (default: the host's) |
| `--env KEY=VALUE` | | set an app environment variable |

The rest of the host environment passes through to the app, so `PORT=8080 DATABASE_URL=... ./my_app start`
works as it would with a normal release. Variables that would break the runtime's layout
(`RELEASE_*`, `ERL_*`, `AMPOULE_*`, `ROOTDIR`, `BINDIR`, `HOME`, ...) are not passed through.

The release's `vm.args` applies as it would with `erl`: `+` flags tune the emulator (`+S`, `+sbwt`,
`+hmax`, ...), `-env` sets a variable and other flags reach `init`. Command-line options win over
`vm.args`. Distribution flags (`-name`, `-sname`, `-setcookie`, ...) are left out with a warning.

For debugging, `AMPOULE_TRACE=1` logs path resolution and `poll_oneoff` calls to stderr, and
`AMPOULE_ENGINE=path/to/beam.wasm` runs a different, non-precompiled engine.

### Configuration

`ampoule.toml` is optional. `ampoule release` reads it from the project root (or `--config`) and
stores it in the binary:

```toml
serve = ["migrate", { run = "seed", when = "SEED=true" }, "start"]   # `<binary> serve`

[commands]                       # `<binary> migrate` evaluates the expression
migrate = "MyApp.Release.migrate()"
seed = "MyApp.Release.seed()"

[env]                            # defaults for every command; the real environment wins
LANG = "C.UTF-8"

[start.env]                      # defaults for `start` only
PHX_SERVER = "true"

[network]
listen = [4000]                  # the only ports the app may bind (default: any)
port_map = { 4000 = 8080 }       # app port -> host port
```

Each `serve` step runs as its own VM, because a guest exit ends the runtime process, and the last
step replaces the process. A `when` condition is `VAR` (set and non-empty) or `VAR=value`. Packing
rejects unknown keys, custom commands that shadow a built-in, and `serve` steps that are neither
`start` nor a `[commands]` entry.

### Signals and shutdown

| Signal | Effect |
|---|---|
| SIGTERM, SIGINT (Ctrl-C) | `init:stop()`: applications stop in order, then the process exits 0. A second signal, or a VM still running after `--stop-timeout`, exits at once with 128 + the signal number. |
| SIGQUIT | `erlang:halt()` |

Code running in wasm cannot receive signals, so the runtime catches them on a dedicated thread and
writes them into ERTS's own signal-dispatcher pipe, where OTP's `erl_signal_server` handles them as
it would on a native build. Anything that stops processes with SIGTERM (`docker stop`, systemd,
Kubernetes) gets a clean shutdown.

### What the app sees

The app sees only these paths:

| Guest path | Backed by |
|---|---|
| `/app` | the embedded release (read-only; files are decompressed when opened) |
| `/tmp` | a private host directory (`--tmp`, or `$TMPDIR/ampoule-<pid>`) |
| `/etc/hosts` | the host's file plus `--host` entries |
| `/etc/resolv.conf` | the host's |
| `/etc/ssl/certs/ca-certificates.crt` | the host's CA bundle (or `AMPOULE_CA_BUNDLE`) |
| `/dev/null` | the host's |
| `/proc` | generated in Linux's formats: `meminfo`, `loadavg`, `stat`, `uptime`, `cpuinfo`, `self/{stat,statm,status,limits,fd}` |
| anything from `--mount` | the host path, optionally read-only |

Symlinks are resolved inside the app's view, never the host's: a link to `/` in `/tmp` points at
the app's own root, and a link to `/etc/passwd` finds nothing unless a mount provides that path.
Host paths are opened one component at a time, so a link created while a lookup is running cannot
redirect it either. Mount points themselves cannot be removed, renamed or replaced.

The guest reports itself as Linux (`:os.type()` is `{:unix, :linux}`) on every host, so OTP and NIFs
take their Linux code paths, including os_mon reading `/proc`.

Without `--tmp`, the runtime removes its private `/tmp` when it exits. On the next start it also
removes directories left by runs that were killed. A directory holding an `erl_crash.dump` is kept,
and its path is printed.

Sockets are host sockets. Binds and connects go through the network policy (`--listen`,
`--port-map`, `--listen-host`, `--rewrite-connect`, `--deny-connect`), and everything else goes
straight to the host. TLS uses OTP's `ssl` with the OpenSSL 3.5 linked into the engine.

### NIFs

`ampoule release` rebuilds native dependencies for wasm and links them into an engine carried in
the binary.

- C and C++ NIFs (rebar3's port compiler, `elixir_make`, cmake) are built by
  `mix deps.compile DEP --force` with `CC` and `CXX` pointing at `ampoule-cc` and `ampoule-c++`.
  This runs on private copies of `deps/` and `_build/` under `_build/ampoule-nif/`, so the project
  itself is left as it was. The wrappers compile for wasm32 against the static NIF API and turn the
  shared-library link into a wasm archive at the same path.
- Rust NIFs (Rustler, in a dependency or in the project's own `native/`) are built with cargo for
  `wasm32-wasip1-threads`, which needs `rustup target add wasm32-wasip1-threads`. Rustler is patched
  to resolve the `enif_*` API statically. Return errors from Rust rather than panicking: on wasm a
  panic aborts the VM.
- setjmp/longjmp and C++ exceptions use wasm exception handling.

Building NIFs needs [wasi-sdk](https://github.com/WebAssembly/wasi-sdk/releases) 34 and the engine
kit. `ampoule` looks for them in `AMPOULE_WASI_SDK` and `AMPOULE_KIT`, then next to its own
executable (`wasi-sdk/`, `../wasi-sdk`, `kit/`), then under `~/.cache/ampoule/`. Release archives
include the kit.

Tested with C (eiconv), C with setjmp/longjmp (libpg_query, through `pg_query_ex`), C++ with
exceptions (snappy, prometheus_process_collector) and Rust (Rustler 0.38). Each dependency can have
one native library.

## Limitations

- Host processes: port programs, `:os.cmd/1` and `System.cmd/3` cannot start anything. For the same
  reason os_mon's `memsup`, `cpu_sup`, `disksup` and `os_sup` are switched off.
- Distribution: the app runs as one node; packing removes distribution from its start arguments,
  and the runtime leaves distribution flags in `vm.args` out.
- NIF threads: a thread a NIF starts must return from its function. `enif_thread_exit` ends the
  thread without the cleanup wasi-libc would do, so joining that thread never returns.
- OTP version: the engine fixes the OTP major, currently OTP 28 (ERTS 16).
- Precompressed static files: packing drops `.gz` and `.br` files, along with sources, docs, `.map`
  files and host binaries. `Plug.Static` falls back to the uncompressed file.
- Debug information: `.beam` files lose their `Dbgi`, `Docs`, `ExCk`, `ExDp` and `Abst` chunks, so
  `Code.fetch_docs/1` and the debugger's source views have nothing to show.

## How it works

`ampoule` carries the BEAM compiled to `wasm32-wasip1-threads` and precompiled to native code
with wasmtime. It is also the operating system the release runs on: WASI preview 1,
`wasi.thread-spawn`, host sockets, a mount table and a generated `/proc`, implemented in Rust over
one fd table that every wasm thread shares.

Packing appends the app to a copy of `ampoule`:

```text
[ampoule][rootfs image][engine?][manifest JSON][rootfs len][engine len][manifest len]["AMPOULE1"]
```

- The rootfs image is the pruned release, one zstd frame per file, with an index read at startup.
- The engine section exists only when the app has NIFs: `beam.wasm` relinked with them and
  precompiled for the binary's target. Without it, the built-in engine runs.
- The manifest holds the name, version, ERTS version, commands, `serve`, environment defaults,
  network policy and NIFs.

At start, the binary finds its payload, loads the precompiled engine, sets up the mount and fd
tables and runs `_start` on a host thread. Each pthread the guest creates becomes a host thread with
its own instance of the module, sharing the same memory and fd table.

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) covers building the engine and the binary, the tests, CI and
releasing.

## License

Apache License 2.0; see [LICENSE](LICENSE). The binaries include Erlang/OTP, OpenSSL, wasmtime and
libraries from wasi-sdk under their own licenses; see [NOTICE](NOTICE).
