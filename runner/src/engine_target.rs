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
