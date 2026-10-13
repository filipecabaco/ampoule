# Contributing to ampoule

## Setup

Everything third-party (wasi-sdk, OTP, OpenSSL) is downloaded at pinned versions with SHA-256
checks and built under `build/`, which is not checked in.

```sh
mise install                 # Erlang/OTP 28 and Elixir (see mise.toml); Rust via runner/rust-toolchain.toml
mise run engine              # build/engine: ERTS for wasm, and the NIF kit   (elixir scripts/engine.exs)
mise run build               # runner/target/release/ampoule                (cargo build --release)
mise run lint                # rustfmt, clippy
mise run test                # unit tests
mise run smoke               # pack test/fixture and check the packed app end to end
mise run examples            # the examples' end-to-end suites
```

`mise.toml` points `AMPOULE_WASI_SDK` and `AMPOULE_KIT` at `build/`. Without mise, put OTP 28 and
Elixir on `PATH`, set those two variables, and run the commands in parentheses.

## The engine

`scripts/engine.exs` builds the engine in steps. Each step is skipped while its inputs are unchanged;
`--force` reruns it.

| Step | Builds |
|---|---|
| `sdk` | downloads wasi-sdk to `build/wasi-sdk` |
| `sources` | downloads OTP and OpenSSL |
| `openssl` | OpenSSL for wasm (`build/openssl`) |
| `aos` | `libaos.a` from `engine/ampoule-os` |
| `erts` | ERTS from a fresh OTP tree with `engine/patches/erts-wasi.patch` applied |
| `kit` | `build/engine/beam.wasm`, `build/engine/erts_vsn` and `build/engine/kit` |

ERTS is always built from a fresh tree. OTP's Makefiles do not track compiler flags, so objects left
over from an earlier configuration keep its feature macros. The patch also fixes OTP's static crypto
build, which otherwise drops configure's `SSL_FLAGS` and compiles out `CRYPTO_memcmp`, the call every
TLS record goes through.

To change ERTS, unpack OTP somewhere, apply the patch, make the change, and write the patch back with
`git diff` (or `diff -ru`) from the OTP root. `engine/ampoule-os/include/ampoule_os.h` is
force-included into every ERTS and NIF compile. It maps the POSIX calls wasi-libc lacks onto the
`ampoule_os` imports that `runner/src/net.rs` implements.

The runner's `build.rs` precompiles `build/engine/beam.wasm` (or `AMPOULE_BEAM`) for the target
and embeds it in the binary.

## Tests

| | What it checks |
|---|---|
| `cargo test` | the runtime's pure logic: paths and mounts, network policy, config, `.beam` stripping, the payload format |
| `scripts/smoke.exs` | packs `test/fixture` (a C NIF) and checks the binary from inside (filesystem, `/proc`, NIF, TCP, TLS) and outside (serving, SIGTERM) |
| `scripts/examples.exs` | each example's `test/e2e` suite against its packed binary |
| `scripts/realtime.exs` | Realtime's own e2e suite against a packed Realtime (`test/realtime/ampoule.toml`); run by hand |

CI (`.github/workflows/ci.yml`) builds the engine once, then on Linux and macOS:

- `test`: lint, unit tests, the release build and the smoke test.
- `examples`: each example's end-to-end suite as its own job, against the binary `test` built.
  `phoenix_ecto` runs on Linux only, where a Postgres service container is available.

## Releasing

Bump `version` in `runner/Cargo.toml`, move the `Unreleased` entries in `CHANGELOG.md` under the new
version, and push a `v*` tag. `.github/workflows/release.yml` builds `ampoule` for all four targets
and publishes a GitHub release with one archive per target (binary, kit, license), the bare binaries
and `SHA256SUMS`.

## Layout

| Path | |
|---|---|
| `runner/` | the `ampoule` crate |
| `engine/patches/erts-wasi.patch` | changes to OTP's ERTS for WASI |
| `engine/ampoule-os/` | the C support library and headers linked into ERTS and NIFs |
| `scripts/` | `engine.exs`, `smoke.exs`, `examples.exs`, `realtime.exs` |
| `examples/` | example apps with end-to-end suites |
| `test/fixture/` | the app `smoke.exs` packs |
| `test/realtime/` | the config `realtime.exs` packs Realtime with |
| `build/` | everything generated (not checked in) |

`runner/src`:

| Module | |
|---|---|
| `main.rs` | the packaging CLI and the app entry point: options, release arguments, VM start, private `/tmp` |
| `pack.rs` | release pruning, `.beam` stripping, rootfs image, payload layout |
| `nif.rs` | rebuilding NIFs for wasm (Mix and cargo) and relinking the engine |
| `toolchain.rs` | wasi-sdk and kit discovery, and the `ampoule-cc`/`c++` compiler wrapper |
| `config.rs` | `ampoule.toml` and the embedded manifest |
| `engine.rs` | wasmtime configuration, shared with `build.rs` |
| `kernel.rs` | fd table, mount table, path resolution |
| `wasi.rs` | `wasi_snapshot_preview1` |
| `net.rs` | the `ampoule_os` module: BSD sockets, pipes, signal registration |
| `signals.rs` | host signals forwarded to ERTS |
| `policy.rs` | the network policy |
| `threads.rs` | `wasi.thread-spawn` and the main thread |
| `proc.rs` | the generated `/proc` |
| `vfs.rs` | the embedded rootfs reader |
| `mem.rs` | access to the shared linear memory |
| `errno.rs` | WASI errno codes and host errno translation |
