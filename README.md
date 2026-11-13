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

The file is your release, unchanged, plus the VM to run it. It behaves like `bin/my_app`: `start`,
`eval`, your own commands such as `migrate`, environment variables for configuration, and a clean
`init:stop()` on SIGTERM. The numbers above come from [`examples/francis`](examples/francis), which
answers its first request about 0.3 seconds after launch.

Phoenix, Ecto, Bandit, Francis and plain OTP applications pack without changes. NIFs in C, C++ and
Rust are rebuilt for wasm while packing; the wasi-sdk and kit that needs are downloaded once per machine (`ampoule setup` does it ahead of time). The app runs in a sandbox: it sees its own release, a
private `/tmp` and only the host paths, ports and programs it is given.

## Install

On macOS or Linux, arm64 or x86_64:

```sh
curl -fsSL https://filipecabaco.github.io/ampoule/install.sh | sh
```

The [script](site/install.sh) installs into `~/ampoule` with the runtime for every platform, checks
each file against the release's checksums, and prints the line to add to your `PATH`. Packing needs
Elixir and Erlang/OTP 28. [Getting started](docs/getting-started.md) covers pinning a version,
installing by hand, and a first app.

## Configure

`ampoule.toml` in the project root is optional, and `ampoule release` packs it into the binary. An
app that migrates its database before it starts:

```toml
serve = ["migrate", "start"]

[commands]
migrate = "MyApp.Release.migrate()"

[start.env]
PHX_SERVER = "true"
```

`<binary> serve` runs the migrations and starts the app. A CLI sets `main` instead, so its users
type `my_cli pull o11y/x`:

```toml
home = "host"
main = "MyCli.main"

[[mounts]]
host = "$PWD"
```

## Documentation

[docs/](docs/README.md) has the guides and the reference:

- [Getting started](docs/getting-started.md)
- Guides: [deploying](docs/guides/deploy.md), [a web app](docs/guides/web-app.md), [a
  CLI](docs/guides/cli.md), [NIFs](docs/guides/nifs.md), [clustering](docs/guides/cluster.md),
  [iPhone and iPad](docs/guides/ios.md)
- Reference: [packaging](docs/reference/packaging.md), [running the packed
  app](docs/reference/running.md), [`ampoule.toml`](docs/reference/configuration.md), [what the app
  sees](docs/reference/sandbox.md), [idle stop, memory and the engine
  cache](docs/reference/runtime.md)
- [Limitations](docs/limitations.md) and [how it works](docs/how-it-works.md)

## For agents

The [`ampoule` skill](skills/ampoule/SKILL.md) teaches a coding agent to pack, configure, run and
debug an app with ampoule. In Claude Code (2.1.275 or later):

```text
/plugin install ampoule --marketplace filipecabaco/ampoule
```

On earlier versions, `/plugin marketplace add filipecabaco/ampoule`, then `/plugin install
ampoule@ampoule`. Other agents can copy `skills/ampoule/` into their skills directory. The site
serves [`llms.txt`](https://filipecabaco.github.io/ampoule/llms.txt), an index of these docs as raw
markdown.

## Examples

[`examples/`](examples) has complete apps, each with an end-to-end suite that packs the app, runs
the binary and tests it from the outside:

| Example | Shows |
|---|---|
| [`phoenix_ecto`](examples/phoenix_ecto) | a Phoenix JSON API on Ecto and Postgres; `serve` creates and migrates the database, then starts |
| [`francis`](examples/francis) | Francis: HTML, JSON, server-sent events, the network policy |
| [`c_nif`](examples/c_nif) | the project's own C NIF in `c_src/`, built by `elixir_make` |
| [`rustler`](examples/rustler) | a Rustler NIF in the project's own `native/` crate |
| [`sqlite`](examples/sqlite) | SQLite through exqlite's C NIF, with the database in a mounted host directory |
| [`ports`](examples/ports) | host programs: `System.cmd/3`, `:os.cmd/1`, a program from `priv/` and a supervised long-running port |
| [`cluster`](examples/cluster) | Erlang distribution without epmd: nodes from one binary find each other; `rpc` and `remote` |

## Contributing

[CONTRIBUTING.md](CONTRIBUTING.md) covers building the engine and the binary, the tests, CI and
releasing.

## License

Apache License 2.0; see [LICENSE](LICENSE). The binaries include Erlang/OTP, OpenSSL, wasmtime and
libraries from wasi-sdk under their own licenses; see [NOTICE](NOTICE).
