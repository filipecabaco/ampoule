# Packaging: the `ampoule` command

| Command | What it does |
|---|---|
| `ampoule release [pack options] [-- mix release args]` | runs `mix release --overwrite` in the current project, then packs it |
| `ampoule pack RELEASE_DIR [pack options]` | packs an existing release directory |
| `ampoule run RELEASE_DIR [--config FILE] [app options] COMMAND` | runs a release directory without packing it, for quick iteration |
| `ampoule version` | runtime version, target and ERTS version |

## Pack options

| Option | Default | |
|---|---|---|
| `--config FILE` | `./ampoule.toml` if present | see [Configuration](configuration.md) |
| `--out DIR` | `_build/<env>/ampoule` | where binaries are written |
| `--target T` | the host's | `aarch64-darwin`, `x86_64-darwin`, `aarch64-linux`, `x86_64-linux`, or `ios` (see [iPhone and iPad](../guides/ios.md)); repeatable |
| `--stub T=FILE` | `$AMPOULE_STUBS/ampoule-T`, else `ampoule-T` next to `ampoule`, else downloaded from the same release | the `ampoule` binary built for target `T` |
| `--allow-native` | off | drop native code (NIFs) instead of rebuilding it or refusing to pack |
| `--env ENV` | `$MIX_ENV`, else `prod` | `MIX_ENV` for `release` |
| `--name NAME` | the only release | which release to build or pack when the project defines several |

Each binary is written as `<out>/<release>-<target>`, for example
`_build/prod/ampoule/my_app-aarch64-linux`.

## Requirements

Packing needs Elixir and Erlang/OTP 28 on the packing machine, since `ampoule release` runs `mix
release`. The release must be built with the same OTP major as the runtime's ERTS (`ampoule
version` shows it; currently OTP 28, ERTS 16), and `pack` refuses a mismatch. Apps with NIFs also
need wasi-sdk and the NIF kit; see [NIFs](../guides/nifs.md).

## The lean runtime

Packed apps start from the lean runtime: `ampoule` without the Cranelift compiler, which a packed
app never needs because its engine is precompiled. Every binary is about 8 MiB smaller for it
(`examples/francis`: 11.8 MiB instead of 20.6). The release archives put it next to `ampoule` as
`ampoule-<target>`. When it is not there, packing starts from `ampoule` itself, and it refuses a
lean runtime from another release.

## Packing for another platform

Packing for another platform uses that platform's lean runtime as the base. Each
[release](https://github.com/filipecabaco/ampoule/releases) publishes them as `ampoule-<target>`.
Name the target:

```sh
ampoule release --target x86_64-linux
```

When `ampoule-x86_64-linux` is not next to `ampoule`, packing downloads it from `ampoule`'s own
release (`v<version>`, never `latest`, so the versions always match), checks it against that
release's `SHA256SUMS`, and keeps it in `~/.cache/ampoule/runtimes/<version>/` (under
`$XDG_CACHE_HOME` when that is set) for the next pack. The download uses `curl`. A file that does
not match its checksum is refused and not kept. `AMPOULE_NO_FETCH=1` turns the download off;
`AMPOULE_RELEASES=URL` downloads from a mirror laid out as `URL/v<version>/ampoule-<target>` with
its `SHA256SUMS`.

Or keep them in one directory and point `AMPOULE_STUBS` at it:
`AMPOULE_STUBS=~/ampoule-stubs ampoule release --target aarch64-darwin --target x86_64-linux`.
NIFs are rebuilt for wasm, so they cross over too: a Mac can pack the Linux binary for a Docker
image.

## What packing leaves out

Packing prunes the release before it compresses it. It drops sources, docs, `.map` files,
precompressed static files (`.gz`, `.br`; `Plug.Static` falls back to the uncompressed file), host
libraries and shell scripts outside `priv/`. `.beam` files lose their `Dbgi`, `Docs`, `ExCk`,
`ExDp` and `Abst` chunks, so `Code.fetch_docs/1` and the debugger's source views have nothing to
show.
