# NIFs

`ampoule release` rebuilds native dependencies for wasm and links them into an engine carried in
the binary. The output shows each one:

```console
==> building checksum for wasm (priv/checksum_nif.so)
==> linking the engine with 1 NIF(s): checksum_nif
_build/prod/ampoule/checksum-aarch64-darwin (15.1 MiB)
```

The packing machine needs wasi-sdk and the NIF kit. The machine that runs the binary needs nothing.

## Setup

Building NIFs needs [wasi-sdk](https://github.com/WebAssembly/wasi-sdk/releases) 34 and the engine
kit. It has to be 34 exactly: the kit is LLVM bitcode, for link-time optimisation of the relinked
engine, and only the LLVM it was built with (23.1.0) can link it. `ampoule` says so if it finds
another. Release archives include the kit as `kit/`.

Nothing to do by hand: packing downloads whichever is missing into `~/.cache/ampoule` (or under
`$XDG_CACHE_HOME`) the first time it needs them, and every project on the machine shares them. To
fetch ahead of time, for CI or an offline machine, run:

```sh
ampoule setup
```

The wasi-sdk is the pinned release from GitHub, checked against its SHA-256, in
`wasi-sdk-34.0/`. The kit comes from this version's own release archive, checked against the
release's `SHA256SUMS`. `AMPOULE_NO_FETCH=1` turns every download off, packing's included. A
development build has no release to take the kit from: build it with `scripts/engine.exs` and set
`AMPOULE_KIT` to `build/engine/kit`.

When one is not found, the error lists every place `ampoule` looked and the fixes.

`ampoule` looks for the wasi-sdk and kit in `AMPOULE_WASI_SDK` and `AMPOULE_KIT`, then next to its
own executable (`wasi-sdk/`, `../wasi-sdk`, `kit/`), then in the cache. To manage the wasi-sdk
yourself, unpack it anywhere and point `AMPOULE_WASI_SDK` at it.

Rust NIFs also need the Rust target: `rustup target add wasm32-wasip1-threads`.

## C and C++

C and C++ NIFs in dependencies (rebar3's port compiler, `elixir_make`, cmake) are built by `mix
deps.compile DEP --force` with `CC` and `CXX` pointing at `ampoule-cc` and `ampoule-c++`. This runs
on private copies of `deps/` and `_build/` under `_build/ampoule-nif/`, so the project itself is
left as it was. The wrappers compile for wasm32 against the static NIF API and turn the
shared-library link into a wasm archive at the same path.

The project's own C or C++ NIF (say `c_src/` built by `elixir_make` and a Makefile) is built the
same way, by `mix compile --force` in a copy of the project. The Makefile only has to use `$(CC)`;
[`examples/c_nif`](../../examples/c_nif) is one.

setjmp/longjmp and C++ exceptions use wasm exception handling.

## Rust

Rust NIFs (Rustler, in a dependency or in the project's own `native/`) are built with cargo for
`wasm32-wasip1-threads`. Rustler is patched to resolve the `enif_*` API statically. Return errors
from Rust rather than panicking: on wasm a panic aborts the VM. [`examples/rustler`](../../examples/rustler)
has a NIF in the project's own `native/` crate.

## The engine cache for packing

The relinked engine is kept in `~/.cache/ampoule/engines/`, under a hash of everything the link
reads: the kit, the wasi-sdk version, the `ampoule` version, and each NIF's built archive. Packing
again with the same NIFs skips the link, which is most of a pack (`examples/c_nif`: 13 s, then 1.3
s), and gives the same binary. The NIFs themselves are still built on every pack. The eight most
recently used engines are kept, and `AMPOULE_NO_CACHE=1` links without the cache. The same variable
also turns off the runtime's [engine cache](../reference/runtime.md#the-engine-cache), for packing
and for running alike.

## What works

Tested with C (eiconv, and SQLite through `exqlite`: see [`examples/sqlite`](../../examples/sqlite)),
C with setjmp/longjmp (libpg_query, through `pg_query_ex`), C++ with exceptions (snappy,
prometheus_process_collector) and Rust (Rustler 0.38). Each dependency can have one native library.

A thread a NIF starts must return from its function: `enif_thread_exit` ends the thread without
the cleanup wasi-libc would do, so joining that thread never returns.

## Without rebuilding

`--allow-native` packs a release whose NIFs cannot be rebuilt by leaving the native code out. The
app then runs without those NIFs, so calls into them fail.
