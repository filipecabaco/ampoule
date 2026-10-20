//! The wasmtime configuration. Shared with build.rs, which precompiles beam.wasm: precompiled
//! code only loads into an engine configured identically. Exceptions carry setjmp/longjmp for
//! NIFs that use them (compiled with -mllvm -wasm-enable-sjlj).

pub fn config() -> wasmtime::Config {
    let mut cfg = wasmtime::Config::new();
    cfg.wasm_threads(true)
        .shared_memory(true)
        .wasm_exceptions(true)
        .max_wasm_stack(8 << 20)
        .async_stack_size(9 << 20)
        .cranelift_opt_level(wasmtime::OptLevel::Speed);
    cfg
}

/// The configuration for precompiling the engine for `triple` (a Rust target triple), on whatever
/// machine builds it. Naming a target gives Cranelift only that architecture's baseline, so the
/// features every machine of the target has are switched on here:
///
/// - aarch64-apple-darwin: LSE, single-instruction atomics, which every Apple Silicon CPU has.
///   The baseline (ARMv8.0) turns each atomic into a load/store-exclusive loop, and ERTS is
///   atomic-heavy: LSE is about 10% on interpreter-bound code. Linux aarch64 keeps the baseline,
///   which ARMv8.0 cores such as the Raspberry Pi 4's need.
pub fn for_target(triple: &str) -> wasmtime::Result<wasmtime::Config> {
    let mut cfg = config();
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
