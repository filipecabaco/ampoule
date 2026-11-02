# A web app

Phoenix, Bandit and Francis apps pack without changes. This guide covers the settings a
long-running server usually needs, from running migrations before `start` to a clean shutdown. [`examples/phoenix_ecto`](../../examples/phoenix_ecto) and
[`examples/francis`](../../examples/francis) are complete apps that do all of it.

## Migrate, then start

Put the order in `ampoule.toml` and start the app with `serve`:

```toml
serve = ["migrate", "start"]

[commands]
migrate = "MyApp.Release.migrate()"

[start.env]
PHX_SERVER = "true"
```

`MyApp.Release.migrate/0` is the module `mix phx.gen.release` generates for a Phoenix app; any
function that runs the migrations works. `./my_app serve` runs `migrate` on a release that is not
started, then starts the app.
`./my_app migrate` runs the migrations on their own. A step can depend on the environment:
`{ run = "seed", when = "SEED=true" }` runs `seed` only when `SEED` is `true`. `[start.env]`
sets defaults for `start` only; a variable set in the real environment wins over it.

## Ports

Without `[network] listen`, the app may bind any port. With it, only those:

```toml
[network]
listen = [4000]
```

Bind the app's port 4000 as host port 8080 without changing the app:

```sh
./my_app --port-map 4000:8080 start
```

`--listen-host` moves wildcard binds (`0.0.0.0`) to one address, and `--deny-connect` and
`--rewrite-connect` act on outgoing connections. See [Network](../reference/sandbox.md#network).

## Configuration through the environment

The host environment passes through, so the usual runtime configuration works:

```sh
PORT=8080 DATABASE_URL=ecto://... SECRET_KEY_BASE=... ./my_app serve
```

Variables that belong to the runtime (`RELEASE_*`, `ERL_*`, `AMPOULE_*`, `HOME`, ...) are not
passed through. `--env KEY=VALUE` sets one for the app explicitly. TLS connections, to a database
or an API, use the host's CA certificates, or `AMPOULE_CA_BUNDLE`.

## Memory

Each VM has at most 4 GiB. Cap it lower with `--memory-max 512M`. At 80% of the cap, `start`
raises OTP's `system_memory_high_watermark` alarm, so code that sheds load on memsup alarms works
as it would natively. To come back from a crash, out of memory included, restart on failure:

```toml
restart = "on-failure"
```

The process keeps its listening sockets open while the VM restarts, so clients wait instead of
being refused. See [Memory and restarts](../reference/runtime.md#memory-and-restarts).

## Stopping when idle

An app that only works in response to requests can stop its VM when nobody is connected:

```toml
idle_stop = "5m"
```

The runtime holds the listening sockets, and the next connection starts the VM again: about 0.3 s
to the first response for `examples/francis` on Linux. An idle app costs about 2 MB. Background
work (timers, scheduled jobs, subscriptions) stops with the VM, so leave this off for apps that
have any. See [Stopping when idle](../reference/runtime.md#stopping-when-idle).

## Shutdown

SIGTERM and Ctrl-C stop the app through `init:stop()`: applications stop in order, and the process
exits 0. A VM still running after 30 seconds (`--stop-timeout`) is stopped at once. `docker stop`,
systemd and Kubernetes all get a clean shutdown. See [Signals and
shutdown](../reference/running.md#signals-and-shutdown).

## Static files

Packing drops precompressed `.gz` and `.br` files, and `Plug.Static` serves the uncompressed file
instead. See [What packing leaves out](../reference/packaging.md#what-packing-leaves-out).
