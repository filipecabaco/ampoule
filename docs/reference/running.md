# Running the packed app

```
<binary> [--vm] [options] <command> [args...]
<binary> [args...]                               # with `main` in ampoule.toml
```

With [`main`](../guides/cli.md), every argument belongs to the app, and `--vm` as the first argument
reaches the commands and options below. `--vm` works without `main` too, so a script that uses it
does not depend on how the binary was packed. `<binary> help` (or `--vm help`) prints this usage
for the binary at hand, with its own `[commands]`.

## Commands

| Command | |
|---|---|
| `start` | start the release (like `bin/my_app start`) |
| `serve` | run the `serve` sequence from the config (default: `start`) |
| `eval EXPR [ARGS...]` | evaluate an expression on a release that is not started (like `bin/my_app eval`); `ARGS` are its `System.argv()` |
| `remote` | a shell on the running node (like `bin/my_app remote`; needs [distribution](../guides/cluster.md)) |
| `rpc EXPR` | evaluate an expression on the running node (like `bin/my_app rpc`; needs [distribution](../guides/cluster.md)) |
| `vm -- ERL_ARGS` | run the VM with raw `erl` arguments; the release is at `/app` |
| `info` | what is packed: the app, its files, the runtime, `main`, commands, NIFs, the network policy, allowed programs, mounts, `home` and distribution |
| `<name> [ARGS...]` | a custom command from `[commands]`; `ARGS` are its `System.argv()` |

`eval`, custom commands and `main` run on a release that is not started, without distribution, as a
native release's `eval` does.

## Options

Each option has an environment-variable form, and a flag wins over its variable. Options go before
the command.

| Flag | Variable | |
|---|---|---|
| `--listen PORTS` | `AMPOULE_LISTEN` | ports the app may bind, e.g. `4000,4001` (default: any) |
| `--port-map G:H,...` | `AMPOULE_PORT_MAP` | bind app port `G` as host port `H` |
| `--listen-host IP` | `AMPOULE_LISTEN_HOST` | the address wildcard binds (`0.0.0.0`) land on |
| `--host NAME=IP,...` | `AMPOULE_HOSTS` | extra `/etc/hosts` entries |
| `--rewrite-connect A=B` | `AMPOULE_REWRITE` | send connections for `A` (ip:port) to `B` |
| `--deny-connect ADDR` | `AMPOULE_DENY` | refuse connections to `ADDR` (ip:port) |
| `--mount H:G[:ro],...` | `AMPOULE_MOUNTS` | expose host path `H` at guest path `G` (replaces `[[mounts]]`) |
| `--allow-exec PROGS` | `AMPOULE_ALLOW_EXEC` | host programs the app may start, e.g. `git,/bin/sh`, or `*` for everything on `PATH` (replaces `[process] allow`; see [Host programs](sandbox.md#host-programs)) |
| `--memory-max SIZE` | `AMPOULE_MEMORY_MAX` | cap the VM's memory, e.g. `512M`, `2G` |
| `--schedulers N` | `AMPOULE_SCHEDULERS` | scheduler threads (default: `+S` in the VM flags, else host cores) |
| `--mode MODE` | `AMPOULE_MODE` | `start`'s code loading: `embedded` (default, every module at boot) or `interactive` (each on first use: less memory for an app that uses little of what it ships) |
| `--vm-args FLAGS` | `AMPOULE_VM_ARGS` | VM flags written as in `vm.args`, e.g. `"+S :4 +SDio 2"` |
| `--restart MODE` | `AMPOULE_RESTART` | with `start`, `on-failure` starts the VM again when it fails, out of memory included (default: `off`; see [Memory and restarts](runtime.md#memory-and-restarts)) |
| `--memory-alarm PCT` | `AMPOULE_MEMORY_ALARM` | with `start`, set the `system_memory_high_watermark` alarm at `PCT`% of the memory limit (default: 80; 0 turns it off) |
| `--idle-stop TIME` | `AMPOULE_IDLE_STOP` | with `start`, stop the VM after `TIME` without connections and start it again on the next one (`300`, `90s`, `5m`, `1h`; see [Stopping when idle](runtime.md#stopping-when-idle)) |
| `--stop-timeout SECS` | `AMPOULE_STOP_TIMEOUT` | how long SIGTERM and SIGINT wait for a clean stop (default: 30) |
| `--node NAME` | `RELEASE_NODE` | run `start` as a distributed node, `NAME` or `NAME@HOST` (see [Clustering](../guides/cluster.md)) |
| `--distribution MODE` | `RELEASE_DISTRIBUTION` | `name`, `sname` or `none` (default: `none`, or what `--node` implies) |
| `--dist-port PORT` | `AMPOULE_DIST_PORT` | the distribution port; without epmd, every node's (default: 9100) |
| `--epmd MODE` | `AMPOULE_EPMD` | `none`: peers are on the distribution port (default); `host`: register with the host's epmd |
| `--cookie-file FILE` | `AMPOULE_COOKIE_FILE` | the distribution cookie (default: `RELEASE_COOKIE`, else the release's own) |
| `--tmp DIR` | `AMPOULE_TMP` | host directory behind `/tmp` (default: a private one) |
| | `AMPOULE_CA_BUNDLE` | CA certificates file (default: the host's) |
| `--native-cache WHERE` | `AMPOULE_NATIVE_CACHE` | where the compiled engine is kept between starts: `auto` (default), `off`, or a directory (see [The engine cache](runtime.md#the-engine-cache)) |
| `--env KEY=VALUE` | | set an app environment variable |

## Environment

The rest of the host environment passes through to the app, so `PORT=8080 DATABASE_URL=...
./my_app start` works as it would with a normal release. Variables that would break the runtime's
layout (`RELEASE_*`, `ERL_*`, `AMPOULE_*`, `ROOTDIR`, `BINDIR`, `HOME`, ...) are not passed
through. `RELEASE_NODE`, `RELEASE_DISTRIBUTION` and `RELEASE_COOKIE` configure
[distribution](../guides/cluster.md) instead. The app's `HOME` is its `/tmp`, unless `home = "host"`
in `ampoule.toml` (see [Configuration](configuration.md#mounts-and-home)).

## VM flags

VM flags apply as they would with `erl`: `+` flags tune the emulator (`+S`, `+sbwt`, `+hmax`, ...),
`-env` sets a variable, and other flags reach `init`. They come from the release's `vm.args`, then
`vm_args` in `ampoule.toml`, then `--vm-args` (or `AMPOULE_VM_ARGS`), and a later flag wins.
`--schedulers` wins over all of them. Inside wasm the VM sees one processor, so scheduler counts
that depend on it (`+S :4`, `+S -2`, `+SDcpu :2`) are resolved against the host's cores.

Distribution flags (`-name`, `-sname`, `-setcookie`, ...) are left out with a warning: the
[distribution](../guides/cluster.md) options set the node. `-hidden`, `-proto_dist`, `-epmd_module`
and `-dist_listen` apply.

## Signals and shutdown

| Signal | Effect |
|---|---|
| SIGTERM, SIGINT (Ctrl-C) | `init:stop()`: applications stop in order, then the process exits 0. A second signal, or a VM still running after `--stop-timeout`, exits at once with 128 + the signal number. |
| SIGQUIT | `erlang:halt()` |

Code running in wasm cannot receive signals, so the runtime catches them on a dedicated thread and
writes them into ERTS's own signal-dispatcher pipe, where OTP's `erl_signal_server` handles them as
it would on a native build. Anything that stops processes with SIGTERM (`docker stop`, systemd,
Kubernetes) gets a clean shutdown.

## Debugging

`AMPOULE_TRACE=1` logs path resolution, `poll_oneoff` calls, refused host programs and the
distribution setup to stderr. `AMPOULE_ENGINE=path/to/beam.wasm` runs a different,
non-precompiled engine. A VM that crashes leaves `erl_crash.dump` in its private `/tmp`, and the
runtime prints that directory's path and keeps it.
