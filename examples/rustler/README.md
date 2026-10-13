# Rustler

An app with a [Rustler](https://hex.pm/packages/rustler) NIF in its own `native/textkit` crate.
`ampoule release` rebuilds the crate for `wasm32-wasip1-threads` and links it into the engine
carried by the binary, so the NIF ships inside the single file like the rest of the app.

```sh
rustup target add wasm32-wasip1-threads    # once
mix deps.get
ampoule release
_build/prod/ampoule/textkit-<target> eval 'IO.puts(TextKit.slugify("Hello, World"))'
_build/prod/ampoule/textkit-<target> demo                # a custom command from ampoule.toml
```

Building NIFs needs wasi-sdk and the engine kit (`AMPOULE_WASI_SDK`, `AMPOULE_KIT`; see the
main README).

- `native/textkit/src/lib.rs`: plain Rustler NIFs, a dirty-CPU NIF, a pure-Rust dependency (`sha2`),
  and a `Result` that becomes `{:ok, _}` / `{:error, _}`.
- Return errors instead of panicking: on wasm a Rust panic aborts the whole VM.

## Tests

`mix test` packs the app and runs the binary (`test/e2e/text_kit_test.exs`): it checks that the NIF
is linked into the packed engine, calls every NIF from inside the binary (including from 2000
concurrent processes and on a dirty scheduler), and runs the custom command.
