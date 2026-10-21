//! Embeds the engine: beam.wasm (AMPOULE_BEAM, default ../build/engine/beam.wasm, produced by
//! scripts/engine.exs) precompiled to native code for the target and compressed. Releases are not
//! embedded here: `ampoule pack` appends them to a copy of the binary.
//!
//! Also records the ERTS version beam.wasm was built from (AMPOULE_ERTS_VSN, default the
//! `erts_vsn` file next to beam.wasm), which releases must match, and the target name used for stubs.

#[path = "src/engine.rs"]
mod engine;
#[path = "src/engine_target.rs"]
mod engine_target;

use std::path::PathBuf;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let target = std::env::var("TARGET").expect("TARGET");
    println!("cargo:rerun-if-env-changed=AMPOULE_BEAM");
    println!("cargo:rerun-if-env-changed=AMPOULE_ERTS_VSN");

    let beam = std::env::var_os("AMPOULE_BEAM").map(PathBuf::from).unwrap_or_else(|| manifest.join("../build/engine/beam.wasm"));
    println!("cargo:rerun-if-changed={}", beam.display());
    let wasm =
        std::fs::read(&beam).unwrap_or_else(|e| panic!("{}: {e}\nbuild the engine first (`elixir scripts/engine.exs`) or set AMPOULE_BEAM", beam.display()));
    let cfg = engine_target::for_target(&target).expect("wasmtime target");
    let native = wasmtime::Engine::new(&cfg).expect("wasmtime engine").precompile_module(&wasm).expect("precompiling beam.wasm");
    std::fs::write(out.join("beam.cwasm.zst"), zstd::bulk::compress(&native, 9).expect("zstd")).expect("writing the engine");

    let erts_vsn = std::env::var("AMPOULE_ERTS_VSN").ok().unwrap_or_else(|| {
        let file = beam.with_file_name("erts_vsn");
        println!("cargo:rerun-if-changed={}", file.display());
        std::fs::read_to_string(&file)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|e| panic!("{}: {e}\nset AMPOULE_ERTS_VSN or build the engine with scripts/engine.exs", file.display()))
    });
    println!("cargo:rustc-env=AMPOULE_ERTS_VSN={erts_vsn}");

    let arch = target.split('-').next().expect("target arch");
    let os = if target.contains("apple-darwin") {
        "darwin"
    } else if target.contains("linux") {
        "linux"
    } else {
        panic!("unsupported target {target}: ampoule runs on macOS and Linux")
    };
    println!("cargo:rustc-env=AMPOULE_TARGET={arch}-{os}");
}
