---
name: ampoule
description: Pack an Elixir or Erlang (Mix) release into one self-contained executable with ampoule, configure it with ampoule.toml, and run or debug the packed binary. Use when the user mentions ampoule, wants to ship a Phoenix/Bandit/Francis/OTP app or an Elixir CLI as a single file, asks about `ampoule release`, `ampoule pack`, `ampoule run`, ampoule.toml keys (serve, commands, main, mounts, home, network, process, distribution), packed binary commands (start, serve, eval, rpc, remote, vm, info, --vm), NIFs for wasm, or errors from a packed app such as :eacces, :enoent or a missing file.
---

# ampoule

ampoule turns a Mix release into one executable. The binary carries the release and the BEAM
compiled to WebAssembly, and runs on macOS or Linux with no Erlang, Elixir or shared libraries.
The app runs in a sandbox: it sees its release, a private `/tmp`, and only the host paths, ports
and programs it is given.

Full documentation: https://github.com/filipecabaco/ampoule/tree/main/docs. The raw pages are at
`https://raw.githubusercontent.com/filipecabaco/ampoule/main/docs/<page>.md`; fetch the one that
matches the task before giving detailed answers (see "Where to look" below).

## Before you start

- The packing machine needs Elixir and Erlang/OTP 28. The release must be built with OTP 28,
  the runtime's major; `ampoule version` shows it, and packing refuses a mismatch.
- Check that ampoule is installed: `ampoule version`. If not, install it. The script detects the
  machine, installs into `~/ampoule` with every platform's runtime (so any `--target` works), and
  checks each file against the release's checksums; `AMPOULE_VERSION=0.1.12` before `sh` pins a
  release:

  ```sh
  curl -fsSL https://filipecabaco.github.io/ampoule/install.sh | sh
  export PATH="$HOME/ampoule:$PATH"
  ```

## Workflow

1. In the project root, run `ampoule release`. It runs `MIX_ENV=prod mix release --overwrite`
   and writes `_build/prod/ampoule/<release>-<target>`.
2. Run the binary as `bin/<release>` would be run: `./my_app start`, `./my_app eval 'EXPR'`,
   `./my_app <command>` for a `[commands]` entry. `./my_app info` shows what is packed, and
   `./my_app help` lists its commands and options.
3. To iterate without packing, build the release and run it in the same sandbox:
   `ampoule run _build/prod/rel/my_app --config ampoule.toml start`.
4. Put settings in `ampoule.toml` at the project root; `ampoule release` packs it into the binary.
   Unknown keys are rejected at pack time.

Run options go before the command and each has an `AMPOULE_*` variable:
`./my_app --port-map 4000:8080 start` equals `AMPOULE_PORT_MAP=4000:8080 ./my_app start`.
The host environment passes through to the app (`PORT=8080 DATABASE_URL=... ./my_app start`),
except variables that belong to the runtime (`RELEASE_*`, `ERL_*`, `AMPOULE_*`, `HOME`, ...).

## Configure for the kind of app

A web app (Phoenix, Bandit, Francis) that migrates before it starts:

```toml
serve = ["migrate", "start"]          # `./my_app serve`: each step is its own VM

[commands]
migrate = "MyApp.Release.migrate()"   # from `mix phx.gen.release`; also `./my_app migrate`

[start.env]
PHX_SERVER = "true"                   # default for `start`; the real environment wins

[network]
listen = [4000]                       # the only ports the app may bind
```

A server can also restart after a crash with `restart = "on-failure"` and cap its memory with
`--memory-max 512M`. `idle_stop = "5m"` stops the VM between connections, which also stops timers
and scheduled jobs, so set it only for apps that do nothing in the background.

A CLI that users run as `my_cli pull o11y/x`:

```toml
home = "host"          # HOME and System.user_home/0 are the host's, so ~/.config works
main = "MyCli.main"    # every argument goes to MyCli.main/1 (binaries) or main/0 (System.argv())

[process]
allow = ["gh"]         # host programs it may start

[[mounts]]
host = "$PWD"          # the directory the user runs it in, at the same path

[[mounts]]
host = "~/.config/my_cli"
create = true          # created with mode 0700 when missing
```

With `main`, the runtime's own commands move behind `--vm` as the first argument:
`my_cli --vm info`, `my_cli --vm eval 'IO.puts(1)'`, `my_cli --vm start`. The app starts in `/`;
call `File.cd(System.get_env("PWD"))` before using relative paths.

Other tasks:

- NIFs (C, C++, Rust/Rustler) are rebuilt for wasm during `ampoule release`. The packing machine
  needs wasi-sdk 34 exactly and the NIF kit (in the release archive as `kit/`); Rust also needs
  `rustup target add wasm32-wasip1-threads`. Return errors from Rust instead of panicking: a panic
  aborts the VM.
- Clustering: `./my_app --node my_app@10.0.0.5 start` with `RELEASE_COOKIE` set. No epmd is
  started; every node listens on port 9100 by default.
- Another platform: `ampoule release --target x86_64-linux`. It uses `ampoule-<target>` from next
  to `ampoule` (the install script adds every platform's); without one, packing downloads it from
  `ampoule`'s own release with `curl`, checks its checksum and caches it.
- A container: `FROM scratch`, copy the Linux binary in, `ENTRYPOINT ["/my_app"]`,
  `CMD ["start"]`. Add CA certificates if the app makes TLS connections.

## Sandbox and runtime rules

- The app sees only `/app` (the release, read-only), a private `/tmp`, `/etc/hosts`,
  `/etc/resolv.conf`, the CA bundle, `/dev/null`, a generated `/proc`, mounts, and allowed
  programs. Anything else on the host does not exist for it.
- Outgoing connections (a database, an API) are allowed unless `--deny-connect` or
  `--rewrite-connect` say otherwise; `[network] listen` limits only the ports the app binds.
- No host program runs unless it is in `[process] allow` (or `--allow-exec`). `:os.cmd/1` and
  `{:spawn, cmd}` ports need `/bin/sh` allowed. Programs in the release's own `priv/` run without
  an entry.
- `HOME` is the private `/tmp` unless `home = "host"`.
- `:os.type()` is `{:unix, :linux}` on every host, and the VM has at most 4 GiB of memory.
- A `[commands]` entry cannot be named after a built-in (`start`, `serve`, `eval`, `remote`,
  `rpc`, `vm`, `info`, `help`).
- `--mount`/`AMPOULE_MOUNTS` replace `[[mounts]]`, and `--allow-exec` replaces `[process] allow`;
  they do not add to them.
- SIGTERM and Ctrl-C stop the app through `init:stop()`, as a native release does.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| a host program fails with `:eacces` | not allowed: add it to `[process] allow` (or `--allow-exec`) |
| a file or directory the app needs gives `:enoent` | not visible in the sandbox: add a `[[mounts]]` entry or `--mount HOST:GUEST` |
| relative paths resolve under `/` | the app starts in `/`: `File.cd(System.get_env("PWD"))` with a `$PWD` mount |
| the app cannot bind a port | the port is not in `[network] listen` / `--listen` |
| `no runtime for <target>: ... could not download ...` | packing could not fetch the runtime: check the network and that `curl` is on `PATH`, or put `ampoule-<target>` from the same release next to `ampoule` (`AMPOULE_NO_FETCH=1` means it never downloads) |
| `... is not the runtime of this ampoule ...: use ampoule-<target> from the same release` | the lean runtime next to `ampoule` is from another version: replace it with the one from the same release, or pass `--stub <target>=FILE` |
| packing refuses the release's ERTS | the release was built with another OTP major: build it with OTP 28 |
| NIF build fails or wrong LLVM | install wasi-sdk 34 next to `ampoule` (as `wasi-sdk/`) or set `AMPOULE_WASI_SDK`; keep `kit/` next to `ampoule` or set `AMPOULE_KIT` |
| TLS connections fail | no CA bundle on the host (e.g. `FROM scratch`): add one, or set `AMPOULE_CA_BUNDLE` |
| the VM crashed | the runtime prints where it kept `erl_crash.dump` (in the run's private `/tmp`) |
| anything else | rerun with `AMPOULE_TRACE=1` to log path resolution, refused programs and distribution setup |

## Where to look

Fetch the relevant page for details beyond this file:

| Task | Page |
|---|---|
| install, first pack, iterate | `docs/getting-started.md` |
| servers, containers, other platforms | `docs/guides/deploy.md` |
| Phoenix and other web apps | `docs/guides/web-app.md` |
| CLIs (`main`, mounts, home) | `docs/guides/cli.md` |
| NIFs | `docs/guides/nifs.md` |
| distribution, `rpc`, `remote` | `docs/guides/cluster.md` |
| iOS | `docs/guides/ios.md` |
| every `ampoule.toml` key | `docs/reference/configuration.md` |
| every run option and variable, signals | `docs/reference/running.md` |
| filesystem, network, host programs | `docs/reference/sandbox.md` |
| idle stop, memory, engine cache | `docs/reference/runtime.md` |
| `ampoule release`/`pack`/`run` options | `docs/reference/packaging.md` |
| what differs from a native release | `docs/limitations.md` |

Complete example apps with end-to-end tests are in
https://github.com/filipecabaco/ampoule/tree/main/examples (phoenix_ecto, francis, c_nif,
rustler, sqlite, ports, cluster).
