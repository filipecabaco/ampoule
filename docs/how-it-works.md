# How it works

`ampoule` carries the BEAM compiled to `wasm32-wasip1-threads` and precompiled to native code with
wasmtime. It is also the operating system the release runs on: WASI preview 1,
`wasi.thread-spawn`, host sockets, host processes, a mount table and a generated `/proc`,
implemented in Rust over one fd table that every wasm thread shares.

## The binary

Packing appends the app to a copy of the lean runtime:

```text
[ampoule][rootfs image][engine?][manifest JSON][rootfs len][engine len][manifest len]["AMPOULE1"]
```

On arm64 macOS the same bytes after `[ampoule]` go in a Mach-O section, `__SUI,__ampoule`, placed
before `__LINKEDIT`, and the binary is signed ad hoc over all of it. A signature has to be the last
thing in a Mach-O, so an appended payload fails `codesign --strict` and can never carry a Developer
ID or be notarized; in a section it can. To ship a packed app signed and notarized:

```console
codesign --force --options runtime --timestamp \
  --entitlements entitlements.plist --sign "Developer ID Application: …" my_app-aarch64-darwin
```

with `com.apple.security.cs.allow-unsigned-executable-memory` in `entitlements.plist`: the
hardened runtime otherwise refuses to make the precompiled engine executable, and the app is killed
at start. Under the hardened runtime the engine cache (`native_cache`) is skipped, because no
entitlement lets it run code mapped from an unsigned file; the engine loads in memory instead.

- The rootfs image is the pruned release, one zstd frame per file, with an index read at startup.
  Every frame is compressed with a zstd dictionary trained on the release's own files and stored
  in the image, so the many small `.beam` files share what they have in common and each file is
  still read on its own.
- The engine section exists only when the app has NIFs: `beam.wasm` relinked with them and
  precompiled for the binary's target. Without it, the built-in engine runs.
- The manifest is the release's name and versions, whether it includes Elixir, the NIFs, and
  `ampoule.toml` as packed: commands, `serve`, `main`, environment defaults, the network and
  process policy, distribution, mounts, `home` and the VM settings.

## Starting

At start, the binary finds its payload, loads the precompiled engine (from the [engine
cache](reference/runtime.md#the-engine-cache) after the first start), sets up the mount and fd
tables, and runs `_start` on a host thread. Each pthread the guest creates becomes a host thread
with its own instance of the module, sharing the same memory and fd table.

Code running in wasm cannot receive signals, so a dedicated host thread catches them and writes
them into ERTS's signal-dispatcher pipe, where OTP's `erl_signal_server` handles them.

[CONTRIBUTING.md](../CONTRIBUTING.md) maps the runtime's source files and explains how the engine
is built.
