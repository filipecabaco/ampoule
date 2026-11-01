<p align="center"><img src="assets/logo.svg" alt="ampoule logo: the bytes 00 61 73 6D that open every WebAssembly module, as four tiles" width="128"></p>

# ampoule

ampoule turns a Mix project into one executable file. Copy the file to a Mac or a Linux machine
and run it: the machine needs no Erlang, no Elixir and no shared libraries.

The project site is at [filipecabaco.github.io/ampoule](https://filipecabaco.github.io/ampoule/).

```console
$ cd my_app
$ ampoule release
==> MIX_ENV=prod mix release --overwrite
==> my_app 0.1.0: 1018 files, 11.6 MiB → 4.6 MiB image (32 paths pruned)
_build/prod/ampoule/my_app-aarch64-darwin (11.8 MiB)
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
publishes them as `ampoule-<target>`. Put the one you need next to `ampoule` and name the target:

```sh
curl -fLo ~/ampoule/ampoule-x86_64-linux https://github.com/filipecabaco/ampoule/releases/latest/download/ampoule-x86_64-linux
ampoule release --target x86_64-linux
```

Or keep them in one directory and point `AMPOULE_STUBS` at it:
`AMPOULE_STUBS=~/ampoule-stubs ampoule release --target aarch64-darwin --target x86_64-linux`.

The Linux binary for a Docker image can come from a Mac this way, NIFs included.

### To iPhone and iPad

An iOS app can start no other process, and a device maps no executable memory, so there is no
binary to pack for iOS. The app links ERTS in instead and boots the release on a thread of its
own; `--target ios` writes what it bundles:

```sh
ampoule release --target ios
# _build/prod/ampoule/my_app-ios/
#   release/                the release, pruned and stripped, without ERTS or host native code
#   liberlang.xcframework   ERTS, OTP's crypto and asn1 NIFs and OpenSSL, for devices and the simulator
#   runtime.json            the OTP, ERTS and minimum iOS it was built for
```

The app calls `erl_start(argc, argv)` with the release's boot script, as its start script would.
[ex_tauri](https://github.com/filipecabaco/ex_tauri)'s `mix ex_tauri.ios device` does all of it for
a Tauri app. The runtime comes with the macOS archives (`ios/`, next to `ampoule`), or from
`AMPOULE_IOS_RUNTIME`; `mise run ios` builds it. Only OTP's own native code runs there: a release
with other NIFs is refused unless `--allow-native`, which leaves them out.

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
==> building checksum for wasm (priv/checksum_nif.so)
==> linking the engine with 1 NIF(s): checksum_nif
_build/prod/ampoule/checksum-aarch64-darwin (15.1 MiB)
```

This needs wasi-sdk on the packing machine (see [NIFs](#nifs)). The machine that runs the binary
needs nothing.

[`examples/`](examples) has complete apps, each with an end-to-end suite that packs the app,
runs the binary and tests it from the outside:

| Example | Shows |
|---|---|
| [`phoenix_ecto`](examples/phoenix_ecto) | Phoenix JSON API on Ecto and Postgres; `serve` creates and migrates the database, then starts |
| [`c_nif`](examples/c_nif) | the project's own C NIF in `c_src/`, built by `elixir_make` |
| [`cluster`](examples/cluster) | Erlang distribution without epmd: nodes from one binary find each other; `rpc` and `remote` |
| [`francis`](examples/francis) | Francis: HTML, JSON, server-sent events, the network policy |
| [`ports`](examples/ports) | host programs: `System.cmd/3`, `:os.cmd/1` and a supervised long-running port |
| [`sqlite`](examples/sqlite) | SQLite through exqlite's C NIF, with the database in a mounted host directory |
| [`rustler`](examples/rustler) | a Rustler NIF in the project's own `native/` crate |

## Install

Download the archive for your machine from the [releases page](https://github.com/filipecabaco/ampoule/releases). There is one for
`aarch64-darwin`, `x86_64-darwin`, `aarch64-linux` and `x86_64-linux`, each holding `ampoule`, the
lean runtime `ampoule-<target>`, the NIF kit (`kit/`) and the license; the macOS ones also hold
the iOS runtime (`ios/`). Keep `kit/`, `ios/` and the lean runtime next to `ampoule`. `ampoule-<target>.tar.gz` is the
latest release, unpacking to `ampoule/`:

```sh
curl -fsSL https://github.com/filipecabaco/ampoule/releases/latest/download/ampoule-aarch64-darwin.tar.gz | tar -xz -C ~
export PATH="$HOME/ampoule:$PATH"
ampoule version
```

`ampoule-<version>-<target>.tar.gz` is the same archive for one version, unpacking to
`ampoule-<version>-<target>/`.

Packed apps start from the lean runtime: `ampoule` without the Cranelift compiler, which a packed
app never needs (its engine is precompiled), so every binary is about 8 MiB smaller
(examples/francis: 11.8 MiB instead of 20.6). When it is not next to `ampoule`, packing starts from
`ampoule` itself, and it refuses a lean runtime from another release. The bare `ampoule-<target>` on
the releases page is that runtime; on its own it packs apps without NIFs:
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
| `--target T` | the host's | `aarch64-darwin`, `x86_64-darwin`, `aarch64-linux`, `x86_64-linux`, or `ios` (see [To iPhone and iPad](#to-iphone-and-ipad)); repeatable |
| `--stub T=FILE` | `$AMPOULE_STUBS/ampoule-T`, else `ampoule-T` next to `ampoule` | the `ampoule` binary built for target `T` |
| `--allow-native` | off | drop native code (NIFs) instead of rebuilding it or refusing to pack |
| `--env ENV` | `$MIX_ENV`, else `prod` | `MIX_ENV` for `release` |
| `--name NAME` | the only release | which release to build or pack when the project defines several |

The release must be built with the same OTP major as the runtime's ERTS (`ampoule version` shows
it; currently OTP 28, ERTS 16). `pack` refuses a mismatch.

### Running the packed app

```
<binary> [options] <command> [args...]
```

| Command | |
|---|---|
| `start` | start the release (like `bin/my_app start`) |
| `serve` | run the `serve` sequence from the config (default: `start`) |
| `eval EXPR [ARGS...]` | evaluate an expression on a release that is not started (like `bin/my_app eval`); `ARGS` are its `System.argv()` |
| `remote` | a shell on the running node (like `bin/my_app remote`; needs [distribution](#distribution)) |
| `rpc EXPR` | evaluate an expression on the running node (like `bin/my_app rpc`; needs [distribution](#distribution)) |
| `vm -- ERL_ARGS` | run the VM with raw `erl` arguments; the release is at `/app` |
| `info` | what is packed: app, file count, runtime, commands, NIFs, network policy, distribution |
| `<name> [ARGS...]` | a custom command from `[commands]`; `ARGS` are its `System.argv()`, so a packed binary can be a CLI |

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
| `--mode MODE` | `AMPOULE_MODE` | `start`'s code loading: `embedded` (default, every module at boot) or `interactive` (each on first use: less memory for an app that uses little of what it ships) |
| `--vm-args FLAGS` | `AMPOULE_VM_ARGS` | VM flags written as in `vm.args`, e.g. `"+S :4 +SDio 2"` |
| `--restart MODE` | `AMPOULE_RESTART` | with `start`, `on-failure` starts the VM again when it fails, out of memory included (default: `off`; see [Memory and restarts](#memory-and-restarts)) |
| `--memory-alarm PCT` | `AMPOULE_MEMORY_ALARM` | with `start`, set the `system_memory_high_watermark` alarm at `PCT`% of the memory limit (default: 80; 0 turns it off) |
| `--idle-stop TIME` | `AMPOULE_IDLE_STOP` | with `start`, stop the VM after `TIME` without connections and start it again on the next one (`300`, `90s`, `5m`, `1h`; see [Stopping when idle](#stopping-when-idle)) |
| `--stop-timeout SECS` | `AMPOULE_STOP_TIMEOUT` | how long SIGTERM and SIGINT wait for a clean stop (default: 30) |
| `--node NAME` | `RELEASE_NODE` | run `start` as a distributed node, `NAME` or `NAME@HOST` (see [Distribution](#distribution)) |
| `--distribution MODE` | `RELEASE_DISTRIBUTION` | `name`, `sname` or `none` (default: `none`, or what `--node` implies) |
| `--dist-port PORT` | `AMPOULE_DIST_PORT` | the distribution port; without epmd, every node's (default: 9100) |
| `--epmd MODE` | `AMPOULE_EPMD` | `none`: peers are on the distribution port (default); `host`: register with the host's epmd |
| `--cookie-file FILE` | `AMPOULE_COOKIE_FILE` | the distribution cookie (default: `RELEASE_COOKIE`, else the release's own) |
| `--tmp DIR` | `AMPOULE_TMP` | host directory behind `/tmp` (default: a private one) |
| | `AMPOULE_CA_BUNDLE` | CA certificates file (default: the host's) |
| `--native-cache WHERE` | `AMPOULE_NATIVE_CACHE` | where the compiled engine is kept between starts: `auto` (default), `off`, or a directory (see [the engine cache](#memory-and-restarts)) |
| `--env KEY=VALUE` | | set an app environment variable |

The rest of the host environment passes through to the app, so `PORT=8080 DATABASE_URL=... ./my_app start`
works as it would with a normal release. Variables that would break the runtime's layout
(`RELEASE_*`, `ERL_*`, `AMPOULE_*`, `ROOTDIR`, `BINDIR`, `HOME`, ...) are not passed through;
`RELEASE_NODE`, `RELEASE_DISTRIBUTION` and `RELEASE_COOKIE` configure [distribution](#distribution)
instead.

VM flags apply as they would with `erl`: `+` flags tune the emulator (`+S`, `+sbwt`, `+hmax`, ...),
`-env` sets a variable and other flags reach `init`. They come from the release's `vm.args`, then
`vm_args` in `ampoule.toml`, then `--vm-args` (or `AMPOULE_VM_ARGS`), and a later flag wins;
`--schedulers` wins over all of them. Inside wasm the VM sees one processor, so scheduler counts
that depend on it (`+S :4`, `+S -2`, `+SDcpu :2`) are resolved against the host's cores.
Distribution flags (`-name`, `-sname`, `-setcookie`, ...) are left out with a warning: the node is
set by the [distribution](#distribution) options.

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
native_cache = "auto"            # keep the compiled engine between starts: "auto", "off", or a directory

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
priv = false                     # programs in the release's own priv/ may not run (default: they may)

[distribution]                   # `start` as a distributed node (default: off)
node = "my_app@10.0.0.5"         # NAME or NAME@HOST; RELEASE_NODE and --node win
mode = "name"                    # "name", "sname" or "none" (default: from `node`)
port = 9100                      # the distribution port
epmd = "none"                    # "none": peers are on `port`; "host": the host's epmd
cookie_file = "/run/secrets/erlang-cookie"
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

The watch looks twice a second once memory is past half the alarm level, and every 5 seconds below
that, so an idle VM spends next to nothing on it; while memory is below half the alarm level, the
alarm can come up to 5 seconds late. It catches memory that grows, not a single allocation that
jumps past the limit. `/proc/meminfo` reports the limit as `MemTotal`, as a container's cgroup limit
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
more of it with more schedulers. To kill a runaway process rather than the VM, cap process heaps with
`+hmax` (and `+hmaxib true` to count binaries) in `vm_args`.

**After a spike.** Memory the VM frees goes back to the host, so the process's RSS falls again
once a spike is over: when ERTS releases a carrier, the runtime drops its pages, keeping at most
32 MiB of them resident for the next carriers to reuse. ERTS's own cache of freed carriers is off
(`+MMmcs 0`): it holds them per scheduler, where under a memory cap they make allocations fail that
the free memory would serve; setting `+MMmcs` in `vm_args` turns it back on. What stays is the VM's
address space: wasm memory cannot shrink, so the range a spike reached remains the VM's, reused for
its later allocations without taking host memory, and `--memory-max` still caps it. Memory from
`malloc` (NIFs, OpenSSL, thread stacks) is reused by the VM but not returned.

**The engine cache.** The binary carries the engine (ERTS compiled to native code) compressed. The
first start writes it out decompressed, and every start after that maps it from that file: its
pages are clean and shared, so the kernel can drop them under pressure and VMs of the same binary
share them, where decompressing into memory kept about 16 MiB private to each VM. The file goes
in the first of `AMPOULE_NATIVE_CACHE`, systemd's `CACHE_DIRECTORY` (a unit with
`CacheDirectory=`; `<dir>/ampoule`), `$XDG_CACHE_HOME/ampoule/native` (an absolute
`XDG_CACHE_HOME` only) and `~/.cache/ampoule/native`. Each engine (one per ampoule version, and
per app with NIFs) takes about 17 MiB, and the four most recently used are kept. The file is named
and checked by the SHA-256 of the code the binary carries, so a damaged or replaced file is never
run but written again; the directory and file must belong to the user running the app, with no
group or other write permission. When the directory cannot be written (a read-only file system)
or refuses executable mappings (a `noexec` mount), the VM loads the engine in memory as before and
says why on stderr; after a failed mapping, a `<hash>.unusable` file in the directory stops later
starts from trying again until it is removed. `AMPOULE_NO_CACHE=1` turns the cache off.

The app chooses when it is packed, with `native_cache` in `ampoule.toml`; `--native-cache` or
`AMPOULE_NATIVE_CACHE` overrides it where it runs:

- `auto` (the default): the directories above, the first that is set.
- `off`: nothing is written, ever; every start loads the engine in memory. For a board whose
  storage should not wear, or a deployment that must not leave files behind. The binary is the
  same single file either way: the cache is only ever made from what it carries.
- a directory: that one. Several instances of the same binary on one host pointing at the same
  directory map one copy of the engine between them, instead of one each: in a test of 1000 nodes
  in containers, about 17 MiB a node instead of 50.

Never mount the cache directory, or a directory containing it, read-write into an app
(`--mount`): the app could change the engine file after it was checked, and another VM mapping it
would run that change as native code, outside wasm. A VM whose own read-write mounts (or `--tmp`)
contain the cache directory does not use it, and says so.

### Distribution

`start` runs the app as one node unless distribution is configured. Naming a node turns it on:

```console
$ RELEASE_COOKIE=... ./my_app --node my_app@10.0.0.5 start
$ RELEASE_COOKIE=... ./my_app --node my_app@10.0.0.5 rpc 'IO.inspect(Node.list())'
[:"my_app@10.0.0.6"]
```

The node is `-name` when its host has a dot or a colon (an FQDN or an IP address) and `-sname`
otherwise; `--distribution` (`RELEASE_DISTRIBUTION`, `mode`) says which, as with `mix release`,
and `none` turns distribution off. `eval` and custom commands run without distribution, as a native
release's `eval` does. The node sees `RELEASE_NODE` and `RELEASE_DISTRIBUTION` in its environment.

**Finding peers.** The runtime starts no epmd. There are two ways to find the other nodes:

- Without epmd (`epmd = "none"`, the default): every node listens on the distribution port, 9100
  unless `port` or `--dist-port` says otherwise, and reaches its peers on that same port. This is
  OTP's `-erl_epmd_port`, which `mix release` documents for deploying without epmd; a release whose
  `vm.args` sets `-erl_epmd_port` gets that port. A native node joins with the same flags:
  `erl -name x@10.0.0.7 -setcookie ... -start_epmd false -erl_epmd_port 9100`.
- With the host's epmd (`epmd = "host"`): the node registers with an epmd already running on the
  host (`127.0.0.1:4369`, or `ERL_EPMD_PORT`), as a native release does, on a free port or on
  `port`. ampoule does not start an epmd, and a machine without Erlang has none to start.

Without epmd, two nodes on one address cannot both have the port. On one machine, give each node
its own host port and send the other's there:

```sh
./my_app --node a@127.0.0.1 --port-map 9100:9101 --rewrite-connect 127.0.0.1:9100=127.0.0.1:9102 start
./my_app --node b@127.0.0.1 --port-map 9100:9102 --rewrite-connect 127.0.0.1:9100=127.0.0.1:9101 start
./my_app --node a@127.0.0.1 --dist-port 9101 rpc 'IO.inspect(Node.connect(:"b@127.0.0.1"))'
```

**The cookie** comes from `--cookie-file` (`AMPOULE_COOKIE_FILE`), then `RELEASE_COOKIE`, then
`cookie_file` in `ampoule.toml`, then the release's own `releases/COOKIE`, as with `mix release`.
That last one is generated when the release is built and packed into the binary, so anyone with the
binary has it: set one of the others in production. The cookie reaches the VM on its command line
inside the runtime, which the host's `ps` does not show, and ampoule never prints it.

**The network policy** applies as to any socket. The distribution port is allowed even when
`--listen` leaves it out, `--port-map` maps it (except with `epmd = "host"`, where epmd would give
peers the app's port), and connections to peers go through `--rewrite-connect` and
`--deny-connect`. Node names must resolve inside the app: an IP address works as it is, the host's
own name (what `-sname` uses) is added to the app's `/etc/hosts` as `127.0.0.1` when the host's
file does not have it, and other names come from the host's `/etc/hosts`, `--host` and DNS.

**`remote` and `rpc`** take the same options as the node and connect to it: they start a hidden VM
from the same binary that does not listen (`-dist_listen false`), with the release's
`remote.vm.args`. When the node's port is mapped, give them the host port with `--dist-port`. As
with a native release, a remote shell whose input ends stops the node it is attached to; leave it
with Ctrl-C.

In the release's `vm.args`, `-name`, `-sname` and `-setcookie` are left out with a warning, and
`-hidden`, `-proto_dist`, `-epmd_module` and `-dist_listen` apply.

### What the app sees

The app sees only these paths:

| Guest path | Backed by |
|---|---|
| `/app` | the embedded release (read-only; files are decompressed when opened) |
| `/tmp` | a private host directory (`--tmp`, or `$TMPDIR/ampoule-<pid>`); its `.ampoule/`, the runtime's own files, is read-only |
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
straight to the host. TLS uses OTP's `ssl` with the OpenSSL 3.5 linked into the engine. UDP
broadcast and multicast work as with any OTP release: `add_membership` joins the group on the host
(an IPv6 link-local group by interface index), and a datagram to a link-local group names its
interface in the destination's `scope_id`. `:inet.getifaddrs/0` lists the host's interfaces.
OTP's `socket` and `net` modules are not in the engine.

Unix domain sockets (`{:local, path}` with `gen_tcp` and `gen_udp`) work at the app's own paths:
binding creates the socket in a directory the app may write (`/tmp`, a writable `--mount`), and
connecting reaches a socket file the app can see, such as a host service mounted in with
`--mount /var/run/app:/run/app`. Nothing outside the mounts is reachable, and Linux's abstract
names (a leading NUL byte) are refused with `:eacces`. A socket path is limited to 104 bytes on
macOS and 108 on Linux as the host sees it; on macOS that includes the private `/tmp`'s own path.

### Host programs

Ports, `System.cmd/3` and `:os.cmd/1` start host programs, but only the ones the app allows in
`[process] allow` (or `--allow-exec`, which replaces that list). Nothing on the host is allowed by
default; programs the release ships in its own `priv/` are (see below).

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

Programs the release ships in its own `priv/` directories run without being allowed: they are the
app's code. Start one by its path, as on a native release:

```elixir
System.cmd(Path.join(:code.priv_dir(:my_app), "bin/tool"), ["--version"])
Port.open({:spawn_executable, Path.join(:code.priv_dir(:my_app), "bin/tool")}, [:binary, :exit_status])
```

The release's files exist only inside the binary, so the first time a program from an app's
`priv/` starts, the runtime extracts that app's whole `priv/` to the private `/tmp`, under
`.ampoule/priv/<app>-<vsn>/`, with the same layout and modes. A program that finds files next to
itself through its own path finds them there. Each app's `priv/` is extracted once per run, and the
app sees the copy read-only, so it cannot change what runs.

- Only a file below `/app/lib/<app>-<vsn>/priv/` qualifies, after symlinks are resolved inside the
  release. A link that leaves the app's `priv/` is not followed when extracting.
- The program runs on the host like any other: the same rules for `cd:`, arguments and environment
  apply, and a `#!` script's interpreter is whatever the host has at that path.
- Packing keeps shell scripts (`.sh`) under `priv/`, where it drops them elsewhere. It also reads the
  header of each executable in `priv/` and warns when one is built for another OS or CPU than the
  target, without failing:
  `==> warning: lib/my_app-0.1.0/priv/bin/tool is a macOS arm64 program; it won't run on x86_64-linux`.
  Build such programs for each target you pack for, or ship a script.
- A shell command line (`:os.cmd/1`, `{:spawn, Cmd}`) goes to the host's `/bin/sh` as it is, and a
  path inside `/app` means nothing there: start priv programs with `System.cmd/3` or
  `{:spawn_executable, Path}`.
- `[process] priv = false` turns this off: then a program in `priv/` fails with `:eacces`, as any
  program that is not allowed does.

### NIFs

`ampoule release` rebuilds native dependencies for wasm and links them into an engine carried in
the binary.

- C and C++ NIFs (rebar3's port compiler, `elixir_make`, cmake) are built by
  `mix deps.compile DEP --force` with `CC` and `CXX` pointing at `ampoule-cc` and `ampoule-c++`.
  This runs on private copies of `deps/` and `_build/` under `_build/ampoule-nif/`, so the project
  itself is left as it was. The wrappers compile for wasm32 against the static NIF API and turn the
  shared-library link into a wasm archive at the same path.
- The project's own C and C++ NIF (say `c_src/` built by `elixir_make` and a Makefile) is built
  the same way, by `mix compile --force` in a copy of the project. The Makefile only has to use
  `$(CC)`; [`examples/c_nif`](examples/c_nif) is one.
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

The relinked engine is kept in `~/.cache/ampoule/engines/`, under a hash of everything the link
reads: the kit, the wasi-sdk version, the `ampoule` version, and each NIF's built archive. Packing
again with the same NIFs skips the link, which is most of a pack (examples/c_nif: 13 s, then
1.3 s), and gives the same binary. The NIFs themselves are still built on every pack. The eight
most recently used engines are kept, and `AMPOULE_NO_CACHE=1` links without the cache. The same
variable also turns off the runtime's [engine cache](#memory-and-restarts), for packing and for
running alike.

Tested with C (eiconv, and SQLite through `exqlite`: see [`examples/sqlite`](examples/sqlite)), C
with setjmp/longjmp (libpg_query, through `pg_query_ex`), C++ with exceptions (snappy,
prometheus_process_collector) and Rust (Rustler 0.38). Each dependency can have one native library.

## Limitations

- Host programs run only when [allowed](#host-programs). Programs in the release's own `priv/` run
  as they were built: one compiled on the machine that packed runs only on hosts of the same OS and
  CPU, and packing warns about the others. os_mon's
  `memsup`, `cpu_sup`, `disksup` and `os_sup` stay switched off, since their programs would run on
  the host and report on it rather than on the VM; the [memory alarm](#memory-and-restarts) stands
  in for `memsup`'s.
- Memory: at most 4 GiB. Freed memory goes back to the host, but the VM keeps the address space a
  spike reached (see [Memory and restarts](#memory-and-restarts)).
- File permissions: files the app creates start at 0644 (directories 0755), less the host's umask,
  as on most hosts natively; `File.chmod/2` then sets the mode on the host file. It cannot set
  setuid or setgid bits, and the owner (`File.stat/1`'s `uid`, `gid`) always reads 0.
- Identity: `:os.type()` is `{:unix, :linux}` and `:erlang.system_info(:logical_processors)` is 1
  on every host. `System.pid/0` is the host pid of the process running the VM (the child, under
  `--restart` or `--idle-stop`).
- Distribution: no epmd is started, so without the host's epmd every node of a cluster uses the
  same distribution port, one node per address (see [Distribution](#distribution)). Clustering
  was tested over TCP (`inet_tcp_dist`) on one machine; TLS distribution was not.
- NIF threads: a thread a NIF starts must return from its function. `enif_thread_exit` ends the
  thread without the cleanup wasi-libc would do, so joining that thread never returns.
- OTP version: the engine fixes the OTP major, currently OTP 28 (ERTS 16).
- Precompressed static files: packing drops `.gz` and `.br` files, along with sources, docs, `.map`
  files, host libraries and shell scripts outside `priv/`. `Plug.Static` falls back to the
  uncompressed file.
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
  Every frame is compressed with a zstd dictionary trained on the release's own files and stored
  in the image, so the many small `.beam` files share what they have in common and each file is
  still read on its own.
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
