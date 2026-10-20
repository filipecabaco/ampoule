# C NIF

An app with its own NIF in C, as one binary: CRC-32 and FNV-1a over binaries, in
`c_src/checksum_nif.c`, built by `elixir_make` and a plain Makefile.

```sh
mix deps.get
ampoule release
_build/prod/ampoule/checksum-<target> eval 'IO.puts(Checksum.crc32("hello"))'
```

- `c_src/checksum_nif.c`: ordinary NIF code, with nothing wasm-specific in it. Inputs over 64 KiB
  go to a dirty CPU scheduler.
- `Makefile`: builds `priv/checksum_nif.so` with `$(CC)`. `ampoule release` runs it again in a copy
  of the project with `CC` pointing at its wasm compiler, which turns the same commands into a wasm
  archive linked into the engine.

Packing needs wasi-sdk 34 (see the ampoule README, NIFs); running needs nothing.

## Tests

`mix test` packs the app and checks the binary (`test/e2e/checksum_test.exs`): CRC-32 against
`:erlang.crc32/1` on small and large (dirty-scheduler) inputs and the standard check value,
FNV-1a against its published test vector, and `ArgumentError` on a bad argument.
