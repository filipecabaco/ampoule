# Getting started

This guide installs ampoule, packs a Mix project into one binary, and runs it. You need Elixir and
Erlang/OTP 28 on the machine that packs. The machine that runs the binary needs nothing.

## Install

On macOS or Linux, arm64 or x86_64:

```sh
curl -fsSL https://filipecabaco.github.io/ampoule/install.sh | sh
export PATH="$HOME/ampoule:$PATH"   # the line it prints; add it to your shell's profile too
ampoule version
```

The [install script](../site/install.sh) finds the machine's OS and CPU, downloads that release's
archive, and adds the lean runtime of every other platform, so `ampoule release --target T` packs
for any of them, offline once installed. It checks every file against the release's `SHA256SUMS`,
needs no sudo and edits no shell profile. Its settings are environment variables:

| Variable | Default | |
|---|---|---|
| `AMPOULE_VERSION` | the latest | a release to install, e.g. `0.1.12` |
| `AMPOULE_HOME` | `~/ampoule` | where to install |
| `AMPOULE_ALL_RUNTIMES` | `1` | `0` installs only this machine's runtime |

```sh
curl -fsSL https://filipecabaco.github.io/ampoule/install.sh | AMPOULE_VERSION=0.1.12 sh
```

Run it again to upgrade: it replaces the files it installed. To uninstall, remove `~/ampoule` and
the `PATH` line.

### Install by hand

Download the archive for your machine from the [releases
page](https://github.com/filipecabaco/ampoule/releases). There is one for `aarch64-darwin`,
`x86_64-darwin`, `aarch64-linux` and `x86_64-linux`. `ampoule-<target>.tar.gz` is the latest
release and unpacks to `ampoule/`:

```sh
curl -fsSL https://github.com/filipecabaco/ampoule/releases/latest/download/ampoule-aarch64-darwin.tar.gz | tar -xz -C ~
export PATH="$HOME/ampoule:$PATH"
ampoule version
```

The archive holds `ampoule`, the lean runtime `ampoule-<target>` that packed apps start from, the
NIF kit (`kit/`), LICENSE, NOTICE, README.md and this documentation (`docs/`). The macOS
archives also hold the iOS runtime (`ios/`). Keep them together: `ampoule` looks for the
runtime, `kit/` and `ios/` next to itself.
`ampoule-<version>-<target>.tar.gz` is the same archive for one version, unpacking to
`ampoule-<version>-<target>/`.

The bare `ampoule-<target>` on the releases page is the lean runtime on its own. Put one next to
`ampoule` to pack for that platform (the install script adds them all). On its own it packs apps
without NIFs:

```sh
curl -fLo ampoule https://github.com/filipecabaco/ampoule/releases/latest/download/ampoule-aarch64-darwin && chmod +x ampoule
```

## Pack an app

Run `ampoule release` in the project root. It runs `mix release` with `MIX_ENV=prod`, then packs
the release:

```console
$ cd my_app
$ ampoule release
==> MIX_ENV=prod mix release --overwrite
==> my_app 0.1.0: 1018 files, 11.6 MiB → 4.6 MiB image (32 paths pruned)
_build/prod/ampoule/my_app-aarch64-darwin (11.8 MiB)
==> packed in 0.3s; run `<binary> help` for its commands
```

The binary is the release, unchanged, plus the VM to run it. Phoenix, Ecto, Bandit, Francis and
plain OTP applications pack without changes.

## Run it

The binary takes the commands `bin/my_app` takes:

```console
$ _build/prod/ampoule/my_app-aarch64-darwin start
[info] Running MyApp with Bandit 1.11.1 at 0.0.0.0:4000 (http)
```

```sh
./my_app eval 'IO.inspect(System.argv())' a b   # an expression, on a release that is not started
./my_app info                                   # what is packed into the binary
./my_app help                                   # its commands and options
```

Configure it with environment variables, as you would a native release: `PORT=8080
DATABASE_URL=... ./my_app start`. Ctrl-C or SIGTERM stops it cleanly through `init:stop()`.

## Iterate without packing

`ampoule run` runs a release directory as the packed binary would, with the same sandbox, so you
can try a change or an `ampoule.toml` without packing:

```sh
MIX_ENV=prod mix release --overwrite
ampoule run _build/prod/rel/my_app --config ampoule.toml start
```

## Configure it

Optional settings live in `ampoule.toml` at the project root, and `ampoule release` packs them into
the binary. An app that migrates its database before it starts writes:

```toml
serve = ["migrate", "start"]

[commands]
migrate = "MyApp.Release.migrate()"

[start.env]
PHX_SERVER = "true"
```

`./my_app serve` then runs the migrations and starts the app, and `./my_app migrate` runs them on
their own. [Configuration](reference/configuration.md) lists every key.

## Next

- [Deploying](guides/deploy.md): a server, an empty container, other platforms.
- [A web app](guides/web-app.md): ports, migrations, idle stop and restarts.
- [A CLI](guides/cli.md): a binary that owns its command line.
- [NIFs](guides/nifs.md): C, C++ and Rust dependencies.
- [Examples](../examples): complete apps with end-to-end suites.
