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
