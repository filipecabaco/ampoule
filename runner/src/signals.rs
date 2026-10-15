//! Host signals for the guest.
//!
//! Signals cannot reach code running in wasm, so the runtime takes them on a dedicated thread and
//! writes them into ERTS's signal-dispatcher pipe (registered by the guest through
//! `ampoule_os.signal_fd`), as ERTS's own signal handler would on a native build. From there OTP
//! handles them as usual:
//!
//! - SIGTERM and SIGINT become the guest's SIGTERM: `init:stop()`, a clean shutdown of the release.
//!   A second one, or a VM still running after the stop timeout, ends the process at once.
//! - SIGQUIT becomes the guest's SIGQUIT: `erlang:halt()`.
//!
//! Before the guest has registered its pipe (early boot, or an engine without the import), a signal
//! ends the process immediately, as it did before the VM started.

use std::sync::Arc;
use std::time::Duration;

use crate::kernel::Kernel;

/// wasi-libc's signal numbers (Linux's), which ERTS was compiled with.
const GUEST_SIGQUIT: i32 = 3;
pub const GUEST_SIGTERM: i32 = 15;

const HANDLED: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGQUIT];

/// The handled signals, blocked in the calling thread and so in every thread it starts afterwards:
/// call before the VM's threads exist, so only `spawn`'s thread receives them.
pub fn block() -> libc::sigset_t {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in HANDLED {
            libc::sigaddset(&mut set, s);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    }
}

/// Start the thread that forwards `set`'s signals to the guest.
pub fn spawn(kernel: Arc<Kernel>, set: libc::sigset_t, stop_timeout: Duration) -> std::io::Result<()> {
    std::thread::Builder::new().name("beam-signals".into()).spawn(move || forward(&kernel, &set, stop_timeout))?;
    Ok(())
}

fn forward(k: &Kernel, set: &libc::sigset_t, stop_timeout: Duration) {
    let mut stopping = false;
    loop {
        let mut sig: libc::c_int = 0;
        if unsafe { libc::sigwait(set, &mut sig) } != 0 {
            continue;
        }
        let guest = if sig == libc::SIGQUIT { GUEST_SIGQUIT } else { GUEST_SIGTERM };
        if stopping || !deliver(k, guest) {
            exit(sig, "exiting");
        }
        if guest == GUEST_SIGTERM {
            stopping = true;
            std::thread::spawn(move || {
                std::thread::sleep(stop_timeout);
                exit(sig, &format!("still running {}s after it, exiting", stop_timeout.as_secs()));
            });
        }
    }
}

/// Write `guest_sig` to the guest's dispatcher pipe; false when there is none yet.
pub fn deliver(k: &Kernel, guest_sig: i32) -> bool {
    let Some(raw) = k.signal_pipe.get().and_then(|e| e.raw()) else { return false };
    let bytes = guest_sig.to_le_bytes();
    unsafe { libc::write(raw, bytes.as_ptr().cast(), bytes.len()) == bytes.len() as isize }
}

fn exit(sig: libc::c_int, what: &str) -> ! {
    let name = match sig {
        libc::SIGTERM => "SIGTERM",
        libc::SIGINT => "SIGINT",
        libc::SIGQUIT => "SIGQUIT",
        _ => "signal",
    };
    eprintln!("ampoule: {name}: {what}");
    // exit(3), not a signal death, so the private /tmp is cleaned up (see main::private_tmp).
    std::process::exit(128 + sig)
}
