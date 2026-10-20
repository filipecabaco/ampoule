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
curl -fsSL https://github.com/filipecabaco/ampoule/releases/download/v0.1.5/ampoule-0.1.5-aarch64-darwin.tar.gz | tar -xz
export PATH="$PWD/ampoule-0.1.5-aarch64-darwin:$PATH"
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
| `--allow-exec PROGS` | `AMPOULE_ALLOW_EXEC` | host programs the app may start, e.g. `git,/bin/sh`, or `*` for everything on `PATH` (replaces `[process] allow`; see [Host programs](#host-programs)) |
| `--memory-max SIZE` | `AMPOULE_MEMORY_MAX` | cap the VM's memory, e.g. `512M`, `2G` |
| `--schedulers N` | `AMPOULE_SCHEDULERS` | scheduler threads (default: `+S` in the VM flags, else host cores) |
| `--vm-args FLAGS` | `AMPOULE_VM_ARGS` | VM flags written as in `vm.args`, e.g. `"+S :4 +SDio 2"` |
| `--restart MODE` | `AMPOULE_RESTART` | with `start`, `on-failure` starts the VM again when it fails, out of memory included (default: `off`; see [Memory and restarts](#memory-and-restarts)) |
| `--memory-alarm PCT` | `AMPOULE_MEMORY_ALARM` | with `start`, set the `system_memory_high_watermark` alarm at `PCT`% of the memory limit (default: 80; 0 turns it off) |
| `--idle-stop TIME` | `AMPOULE_IDLE_STOP` | with `start`, stop the VM after `TIME` without connections and start it again on the next one (`300`, `90s`, `5m`, `1h`; see [Stopping when idle](#stopping-when-idle)) |
| `--stop-timeout SECS` | `AMPOULE_STOP_TIMEOUT` | how long SIGTERM and SIGINT wait for a clean stop (default: 30) |
| `--tmp DIR` | `AMPOULE_TMP` | host directory behind `/tmp` (default: a private one) |
| | `AMPOULE_CA_BUNDLE` | CA certificates file (default: the host's) |
| `--env KEY=VALUE` | | set an app environment variable |

The rest of the host environment passes through to the app, so `PORT=8080 DATABASE_URL=... ./my_app start`
works as it would with a normal release. Variables that would break the runtime's layout
(`RELEASE_*`, `ERL_*`, `AMPOULE_*`, `ROOTDIR`, `BINDIR`, `HOME`, ...) are not passed through.

VM flags apply as they would with `erl`: `+` flags tune the emulator (`+S`, `+sbwt`, `+hmax`, ...),
`-env` sets a variable and other flags reach `init`. They come from the release's `vm.args`, then
`vm_args` in `ampoule.toml`, then `--vm-args` (or `AMPOULE_VM_ARGS`), and a later flag wins;
`--schedulers` wins over all of them. Inside wasm the VM sees one processor, so scheduler counts
that depend on it (`+S :4`, `+S -2`, `+SDcpu :2`) are resolved against the host's cores.
Distribution flags (`-name`, `-sname`, `-setcookie`, ...) are left out with a warning.

For debugging, `AMPOULE_TRACE=1` logs path resolution and `poll_oneoff` calls to stderr, and
`AMPOULE_ENGINE=path/to/beam.wasm` runs a different, non-precompiled engine.

### Configuration

`ampoule.toml` is optional. `ampoule release` reads it from the project root (or `--config`) and
stores it in the binary:

```toml
serve = ["migrate", { run = "seed", when = "SEED=true" }, "start"]   # `<binary> serve`
vm_args = "+S :4 +SDio 2"        # VM flags as in vm.args, after the release's own
idle_stop = "5m"                 # stop the VM after 5 minutes without connections
restart = "on-failure"           # start the VM again when it fails (a crash, out of memory)
memory_alarm = 80                # memory alarm at this % of the VM's memory limit (0: off)

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

[process]
allow = ["git", "/bin/sh"]       # host programs the app may start (default: none; "*": all on PATH)
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

### Stopping when idle

With `--idle-stop TIME` (or `AMPOULE_IDLE_STOP`, or `idle_stop` in `ampoule.toml`), `start` stops
the VM after `TIME` without connections and starts it again when the next one arrives, so an idle
app costs only the runtime's own process:

```console
$ ./my_app --idle-stop 5m start
ampoule: stopping the VM after 5m without connections
...
ampoule: no connections for 5m, stopping the VM until the next one
ampoule: VM stopped; waiting for a connection on port 4000
ampoule: connection on port 4000, starting the VM
```

The process that runs `start` holds the app's listening sockets and runs the VM in a child process.
When the VM stops, the sockets stay open: a new connection waits in the listen backlog while the
VM boots, and is accepted as soon as the app listens again, so none is refused. With `serve`, the
earlier steps run once and only `start` runs again.

| Realtime, measured | Idle | Waking |
|---|---|---|
| macOS, aarch64 | 2 MB, against about 150-250 MB running | first response 0.8 s after a request on a stopped VM |
| Linux, aarch64 (francis) | 1.6 MB | first response 0.3 s |

- Only connections to ports the app listens on with a fixed number count; outgoing ones (a
  database, an API) do not. An open connection, such as a WebSocket, keeps the VM running.
- Background work stops with the VM: timers, scheduled jobs, outgoing connections, subscriptions.
  Use it for apps that only work in response to connections.
- SIGTERM and SIGINT go to the running VM as usual; with the VM stopped, the process exits at once.
- A connection that arrives at the moment the VM decides to stop can be accepted by an app that is
  shutting down, and dropped.

### Memory and restarts

The VM's memory is capped: by `--memory-max`, or at 4 GiB, the most wasm32 can address. As on a
native release, an allocation the VM cannot make ends it with a crash dump
(`binary_alloc: Cannot allocate ...`). ampoule gives the app two ways to deal with that.

**The memory alarm.** `start` watches `:erlang.memory(:total)` against the limit and sets OTP's
standard `{:system_memory_high_watermark, []}` alarm through `:alarm_handler` at 80% (`memory_alarm`,
`--memory-alarm`), clearing it 10 points lower. It is the alarm os_mon's memsup raises, which cannot
run here, so code that already sheds load on memsup alarms works unchanged:

```elixir
:gen_event.add_handler(:alarm_handler, MyApp.MemoryAlarm, [])   # handle_event({:set_alarm, ...})
```

The watch looks twice a second, so it catches memory that grows, not a single allocation that jumps
past the limit. `/proc/meminfo` reports the limit as `MemTotal`, as a container's cgroup limit
shows, for code that sizes itself from it.

**Restarting.** With `restart = "on-failure"` (or `--restart on-failure`), `start` runs the VM as a
child and starts it again when it exits with a non-zero status, after 1, 2, 4, 8 and 16 seconds.
Listening sockets stay open in between (as with [`--idle-stop`](#stopping-when-idle)), so clients
wait instead of being refused. After 5 failures within 5 minutes it gives up and exits with the VM's
status, for systemd, Kubernetes or Docker to take over. A clean stop (`System.stop/0`, SIGTERM) is
not restarted. Each failure leaves its crash dump in the run's `/tmp`.

```console
$ ./my_app --memory-max 256M --restart on-failure start
[warning] ampoule: VM memory at 81% of its 256 MiB limit; alarm system_memory_high_watermark set
binary_alloc: Cannot allocate 16777216 bytes of memory (of type "binary").
ampoule: VM failed (status 1); restarting in 1s (1 of 5 in 5m)
```

Usable memory is less than the limit: thread stacks and allocator overhead live in the same memory,
more of it with more schedulers. The VM's memory also never shrinks once grown, so a spike stays
reserved until the VM restarts. To kill a runaway process rather than the VM, cap process heaps with
`+hmax` (and `+hmaxib true` to count binaries) in `vm_args`.

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
| each program in `[process] allow` | the host's, read-only, at the path it has on the host |

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

Unix domain sockets (`{:local, path}` with `gen_tcp` and `gen_udp`) work at the app's own paths:
binding creates the socket in a directory the app may write (`/tmp`, a writable `--mount`), and
connecting reaches a socket file the app can see, such as a host service mounted in with
`--mount /var/run/app:/run/app`. Nothing outside the mounts is reachable, and Linux's abstract
names (a leading NUL byte) are refused with `:eacces`. A socket path is limited to 104 bytes on
macOS and 108 on Linux as the host sees it; on macOS that includes the private `/tmp`'s own path.

### Host programs

Ports, `System.cmd/3` and `:os.cmd/1` start host programs, but only the ones the app allows in
`[process] allow` (or `--allow-exec`, which replaces that list). Nothing is allowed by default.

```toml
[process]
allow = ["git", "/bin/sh"]   # a name is looked up on PATH; "*" allows everything on PATH
```

```elixir
System.cmd("git", ["rev-parse", "HEAD"])      # {"1f3c...\n", 0}
:os.cmd(~c"date +%s")                          # needs /bin/sh, which :os.cmd runs
Port.open({:spawn_executable, "/bin/sh"}, [:binary, :exit_status, args: ["-c", "cat"]])
```

Each allowed program is also mounted read-only at its host path, so `System.find_executable/1`
finds it. Programs missing on the host are left out, and the app gets the same `:enoent` it would
get natively. A program that is not allowed fails with `:eacces`.

- The program runs on the host, outside the app's view of the filesystem. Its path and working
  directory (`cd:`) are translated from the app's paths: `cd: "/tmp"` runs it in the host directory
  behind `/tmp`. Arguments and environment pass through unchanged, except `HOME`, which points at the
  host directory behind the app's `/tmp`. A path inside `/app` means nothing to a host program.
- Allowing `/bin/sh` (which `:os.cmd/1` and `{:spawn, Cmd}` ports need) allows whatever the shell
  can start. On busybox systems, allowing one applet allows all of them, since the app chooses
  `argv[0]`.
- An empty `FROM scratch` image has no programs to run, not even `/bin/sh`.
- Exit statuses, `os_pid`, `stderr_to_stdout`, `nouse_stdio` and `{:packet, N}` behave as on a
  native release.

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
kit. It has to be 34 exactly: the kit is LLVM bitcode, for link-time optimisation of the relinked
engine, and only the LLVM it was built with (23.1.0) can link it; `ampoule` says so if it finds
another. `ampoule` looks for them in `AMPOULE_WASI_SDK` and `AMPOULE_KIT`, then next to its own
executable (`wasi-sdk/`, `../wasi-sdk`, `kit/`), then under `~/.cache/ampoule/`. Release archives
include the kit.

Tested with C (eiconv), C with setjmp/longjmp (libpg_query, through `pg_query_ex`), C++ with
exceptions (snappy, prometheus_process_collector) and Rust (Rustler 0.38). Each dependency can have
one native library.

## Limitations

- Host programs run only when [allowed](#host-programs). Port programs inside the release (`priv/`)
  are not packed, so they cannot run. For the same reason os_mon's `memsup`, `cpu_sup`, `disksup`
  and `os_sup` are switched off; the [memory alarm](#memory-and-restarts) stands in for `memsup`'s.
- Memory: at most 4 GiB, and it never shrinks back once grown (see
  [Memory and restarts](#memory-and-restarts)).
- File permissions: files the app creates start at 0644 (directories 0755), less the host's umask,
  as on most hosts natively; `File.chmod/2` then sets the mode on the host file. It cannot set
  setuid or setgid bits, and the owner (`File.stat/1`'s `uid`, `gid`) always reads 0.
- Identity: `:os.type()` is `{:unix, :linux}` and `:erlang.system_info(:logical_processors)` is 1
  on every host. `System.pid/0` is the host pid of the process running the VM (the child, under
  `--restart` or `--idle-stop`).
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
`wasi.thread-spawn`, host sockets, host processes, a mount table and a generated `/proc`,
implemented in Rust over one fd table that every wasm thread shares.

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
