//! wasi.thread-spawn: every pthread the guest creates becomes a host thread running a fresh
//! instance of the same module on the same shared memory and kernel.

use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};

use wasmtime::{Engine, Linker, Module, Store, WasmParams};

use crate::kernel::Kernel;
use crate::Ctx;

/// Raised by ampoule_os.thread_exit (pthread_exit) to unwind the calling wasm thread.
#[derive(Debug)]
pub struct ThreadExit;

impl std::fmt::Display for ThreadExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("thread exit")
    }
}

impl std::error::Error for ThreadExit {}

pub struct Spawner {
    pub engine: Engine,
    pub module: Module,
    pub linker: Arc<Linker<Ctx>>,
    pub kernel: Arc<Kernel>,
}

pub static SPAWNER: OnceLock<Spawner> = OnceLock::new();

const HOST_STACK: usize = 16 << 20;

/// Exit status for a guest that trapped (what SIGABRT would give).
const TRAPPED: i32 = 134;

pub fn add(linker: &mut Linker<Ctx>) -> anyhow::Result<()> {
    linker.func_wrap("wasi", "thread-spawn", |_c: wasmtime::Caller<'_, Ctx>, start_arg: i32| -> i32 {
        // wasi-threads: a positive tid, or a negated errno.
        let Some(s) = SPAWNER.get() else { return -crate::errno::AGAIN };
        let Some(tid) = s.kernel.tid() else { return -crate::errno::AGAIN };
        let spawned = std::thread::Builder::new().name(format!("beam-{tid}")).stack_size(HOST_STACK).spawn(move || run(s, tid, start_arg));
        match spawned {
            Ok(_) => tid,
            Err(_) => -crate::errno::AGAIN,
        }
    })?;
    Ok(())
}

/// A fresh instance of the module on its own store, calling `export(params)`. A `ThreadExit`
/// unwinding out of it counts as a normal return.
fn call<P: WasmParams>(s: &Spawner, export: &str, params: P) -> wasmtime::Result<()> {
    let mut store = Store::new(&s.engine, Ctx { k: s.kernel.clone() });
    let instance = s.linker.instantiate(&mut store, &s.module)?;
    match instance.get_typed_func::<P, ()>(&mut store, export)?.call(&mut store, params) {
        Err(e) if e.is::<ThreadExit>() => Ok(()),
        r => r,
    }
}

fn run(s: &Spawner, tid: i32, start_arg: i32) {
    struct Live<'a>(&'a Kernel);
    impl Drop for Live<'_> {
        fn drop(&mut self) {
            self.0.live_threads.fetch_sub(1, Ordering::Relaxed);
        }
    }
    s.kernel.live_threads.fetch_add(1, Ordering::Relaxed);
    let _live = Live(&s.kernel);
    // A panic in a host function must end the process, not just this thread: the guest thread
    // may hold VM locks, and the rest of the VM would wait on it forever.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call(s, "wasi_thread_start", (tid, start_arg)))) {
        Ok(Ok(())) => {},
        Ok(Err(e)) => {
            eprintln!("ampoule: thread {tid} trapped: {e:?}");
            std::process::exit(TRAPPED);
        },
        Err(_) => {
            eprintln!("ampoule: thread {tid}: the runtime panicked");
            std::process::exit(TRAPPED);
        },
    }
}

/// Run the guest's `_start` on a host thread with a large stack; returns its exit status.
pub fn run_main(s: &'static Spawner) -> i32 {
    let main = std::thread::Builder::new().name("beam-main".into()).stack_size(HOST_STACK).spawn(move || match call(s, "_start", ()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("ampoule: trapped: {e:?}");
            TRAPPED
        },
    });
    match main {
        Ok(h) => h.join().unwrap_or(TRAPPED),
        Err(e) => {
            eprintln!("ampoule: cannot start the main thread: {e}");
            TRAPPED
        },
    }
}
