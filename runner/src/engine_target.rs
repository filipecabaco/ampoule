//! The configuration for compiling the engine for a target (Cranelift settings). build.rs always
//! uses it; the runtime only with the `compiler` feature, to precompile engines relinked with NIFs.

/// The configuration for precompiling the engine for `triple` (a Rust target triple), on whatever
/// machine builds it. Naming a target gives Cranelift only that architecture's baseline, so the
/// features every machine of the target has are switched on here:
///
/// - aarch64-apple-darwin: LSE, single-instruction atomics, which every Apple Silicon CPU has.
///   The baseline (ARMv8.0) turns each atomic into a load/store-exclusive loop, and ERTS is
///   atomic-heavy: LSE is about 10% on interpreter-bound code. Linux aarch64 keeps the baseline,
///   which ARMv8.0 cores such as the Raspberry Pi 4's need.
pub fn for_target(triple: &str) -> wasmtime::Result<wasmtime::Config> {
    let mut cfg = crate::engine::config();
    cfg.target(triple)?;
    if triple == "aarch64-apple-darwin" {
        // SAFETY: the flag names a CPU feature every machine this triple runs on has; wasmtime
        // also checks the host has it before running code compiled with it.
        unsafe {
            cfg.cranelift_flag_enable("has_lse");
        }
    }
    Ok(cfg)
}

/// beam.wasm compiled to native code for `triple`, as a binary carries it: the SHA-256 of the
/// native code, then the code zstd-compressed. The digest is what the runtime checks a cached
/// copy against before mapping it as code (native_cache), so it has to come from here, with the
/// code, and not from the cache.
pub fn precompile(wasm: &[u8], triple: &str) -> wasmtime::Result<Vec<u8>> {
    use sha2::Digest;
    let native = wasmtime::Engine::new(&for_target(triple)?)?.precompile_module(wasm)?;
    let mut packed = sha2::Sha256::digest(&native).to_vec();
    // Level 19: about 11% smaller than 9 for the same decompression time at startup; only the
    // build or the pack pays (a few seconds).
    packed.extend(zstd::bulk::compress(&native, 19)?);
    Ok(packed)
}
