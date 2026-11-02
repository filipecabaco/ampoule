# ampoule documentation

Start with [Getting started](getting-started.md): it installs ampoule, packs an app and runs it.

## Guides

Each guide covers one kind of app or one task, from `ampoule.toml` to the commands that run it.

| Guide | For |
|---|---|
| [Getting started](getting-started.md) | installing, packing a first app, running it, iterating with `ampoule run` |
| [Deploying](guides/deploy.md) | a server, an empty `FROM scratch` container, packing for other platforms |
| [A web app](guides/web-app.md) | Phoenix and Bandit apps: migrations, ports, memory, idle stop, shutdown |
| [A CLI](guides/cli.md) | a binary that owns its command line, sees the user's files and finds their config |
| [NIFs](guides/nifs.md) | C, C++ and Rust dependencies, and what packing them needs |
| [Clustering](guides/cluster.md) | Erlang distribution without epmd, the cookie, `remote` and `rpc` |
| [iPhone and iPad](guides/ios.md) | bundling a release and ERTS into an iOS app |

## Reference

| Page | Covers |
|---|---|
| [Packaging](reference/packaging.md) | `ampoule release`, `pack`, `run` and their options; the lean runtime; cross-platform packing |
| [Running the packed app](reference/running.md) | the binary's commands, every option and variable, VM flags, signals, debugging |
| [Configuration](reference/configuration.md) | every key in `ampoule.toml` |
| [What the app sees](reference/sandbox.md) | the filesystem, the network policy, host programs |
| [Idle stop, memory and the engine cache](reference/runtime.md) | tuning a long-running `start` |
| [Limitations](limitations.md) | what differs from a native release |
| [How it works](how-it-works.md) | the binary's layout and how the runtime starts |

## Examples

[`examples/`](../examples) has complete apps, each with an end-to-end suite that packs the app,
runs the binary and tests it from the outside:

| Example | Shows |
|---|---|
| [`phoenix_ecto`](../examples/phoenix_ecto) | a Phoenix JSON API on Ecto and Postgres; `serve` creates and migrates the database, then starts |
| [`francis`](../examples/francis) | Francis: HTML, JSON, server-sent events, the network policy |
| [`c_nif`](../examples/c_nif) | the project's own C NIF in `c_src/`, built by `elixir_make` |
| [`rustler`](../examples/rustler) | a Rustler NIF in the project's own `native/` crate |
| [`sqlite`](../examples/sqlite) | SQLite through exqlite's C NIF, with the database in a mounted host directory |
| [`ports`](../examples/ports) | host programs: `System.cmd/3`, `:os.cmd/1`, a program from `priv/` and a supervised long-running port |
| [`cluster`](../examples/cluster) | Erlang distribution without epmd: nodes from one binary find each other; `rpc` and `remote` |
