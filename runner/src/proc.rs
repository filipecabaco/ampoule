//! The guest's /proc: generated when opened, in Linux's formats, so OTP (os_mon's memsup and
//! cpu_sup) and NIFs that read /proc on Linux work.
//!
//! /proc/self describes the VM: the runtime process's CPU time, the wasm threads, the wasm
//! memory, and the guest's own fd table. The system-wide files (meminfo, loadavg, stat, uptime,
//! cpuinfo) come from the host: its own /proc on Linux, Mach and sysctl on macOS.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::kernel::Kernel;

const TICKS: u64 = 100;
/// sysconf(_SC_PAGESIZE) in the guest: the wasm page.
const PAGE: u64 = 65536;

pub const DIRS: &[&str] = &["/", "/self", "/self/fd"];

struct Host {
    mem_total: u64,
    mem_free: u64,
    mem_available: u64,
    /// user, nice, system, idle, in ticks.
    cpu: [u64; 4],
    boot_time: u64,
    loadavg: [f64; 3],
}

fn now_secs() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn loadavg() -> [f64; 3] {
    let mut l = [0f64; 3];
    // SAFETY: room for the three averages asked for.
    unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    l
}

#[cfg(target_os = "linux")]
fn host() -> Host {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb = |key: &str| -> u64 {
        meminfo.lines().find_map(|l| l.strip_prefix(key)).and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok()).unwrap_or(0) * 1024
    };
    let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
    let cpu: Vec<u64> = stat.lines().next().unwrap_or("").split_whitespace().skip(1).filter_map(|v| v.parse().ok()).collect();
    let boot_time = stat.lines().find_map(|l| l.strip_prefix("btime ")).and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    Host {
        mem_total: kb("MemTotal:"),
        mem_free: kb("MemFree:"),
        mem_available: kb("MemAvailable:"),
        cpu: [cpu.first().copied().unwrap_or(0), cpu.get(1).copied().unwrap_or(0), cpu.get(2).copied().unwrap_or(0), cpu.get(3).copied().unwrap_or(0)],
        boot_time,
        loadavg: loadavg(),
    }
}

#[cfg(target_os = "macos")]
#[allow(deprecated)] // libc points at the mach2 crate for mach_host_self; one call does not warrant it
fn host() -> Host {
    /// # Safety
    /// `T` must be the plain-data type the kernel stores under `name`.
    unsafe fn sysctl<T: Default>(name: &str) -> T {
        let mut v = T::default();
        let mut len = std::mem::size_of::<T>();
        let c = std::ffi::CString::new(name).unwrap();
        libc::sysctlbyname(c.as_ptr(), &mut v as *mut T as *mut libc::c_void, &mut len, std::ptr::null_mut(), 0);
        v
    }
    // SAFETY: each sysctl is read as the type the kernel stores under its name, and Mach's
    // statistics into zeroed plain-data structs with their own counts. No safe crate in the tree
    // wraps these.
    unsafe {
        let mem_total: u64 = sysctl("hw.memsize");
        let page: u64 = libc::sysconf(libc::_SC_PAGESIZE) as u64;
        let host = libc::mach_host_self();
        let mut vm: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        libc::host_statistics64(host, libc::HOST_VM_INFO64, &mut vm as *mut _ as *mut i32, &mut count);
        let free = vm.free_count as u64 * page;
        let available = (vm.free_count as u64 + vm.inactive_count as u64 + vm.purgeable_count as u64) * page;
        let mut ticks = [0u32; 4];
        let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
        libc::host_statistics(host, libc::HOST_CPU_LOAD_INFO, ticks.as_mut_ptr() as *mut i32, &mut count);
        let boot: libc::timeval = sysctl("kern.boottime");
        // Mach's order is user, system, idle, nice.
        Host {
            mem_total,
            mem_free: free,
            mem_available: available,
            cpu: [ticks[0] as u64, ticks[3] as u64, ticks[1] as u64, ticks[2] as u64],
            boot_time: boot.tv_sec as u64,
            loadavg: loadavg(),
        }
    }
}

fn rusage() -> (u64, u64, u64, u64) {
    // SAFETY: rusage is plain data; getrusage fills it. (rustix has no getrusage.)
    let r: libc::rusage = unsafe {
        let mut r = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_SELF, &mut r);
        r
    };
    let ticks = |t: libc::timeval| t.tv_sec as u64 * TICKS + t.tv_usec as u64 * TICKS / 1_000_000;
    (ticks(r.ru_utime), ticks(r.ru_stime), r.ru_minflt as u64, r.ru_majflt as u64)
}

/// `/<pid>/...` is `/self/...`: the guest is one process, and its getpid is this process's.
fn canonical(path: &str) -> String {
    let rest = path.trim_start_matches('/');
    let (first, tail) = rest.split_once('/').map(|(a, b)| (a, format!("/{b}"))).unwrap_or((rest, String::new()));
    if !first.is_empty() && first.bytes().all(|b| b.is_ascii_digit()) {
        format!("/self{tail}")
    } else {
        path.to_string()
    }
}

#[derive(Clone, Copy)]
pub enum Entry {
    Dir,
    File,
    /// /proc/self/fd/N are symlinks on Linux, and readers count them as such.
    Link,
}

pub fn is_dir(path: &str) -> bool {
    DIRS.contains(&canonical(path).as_str())
}

/// Directory entries of a /proc directory.
pub fn list(k: &Kernel, path: &str) -> Option<Vec<(String, Entry)>> {
    let files = |names: &[&str]| names.iter().map(|n| (n.to_string(), Entry::File)).collect::<Vec<_>>();
    match canonical(path).as_str() {
        "/" => Some([("self".to_string(), Entry::Dir)].into_iter().chain(files(&["meminfo", "loadavg", "stat", "uptime", "cpuinfo"])).collect()),
        "/self" => Some(files(&["stat", "statm", "status", "limits"]).into_iter().chain([("fd".to_string(), Entry::Dir)]).collect()),
        "/self/fd" => Some(open_fds(k).into_iter().map(|fd| (fd.to_string(), Entry::Link)).collect()),
        _ => None,
    }
}

fn open_fds(k: &Kernel) -> Vec<usize> {
    k.fds.read().unwrap().iter().enumerate().filter(|(_, e)| e.is_some()).map(|(i, _)| i).collect()
}

/// The contents of a /proc file.
pub fn read(k: &Kernel, path: &str) -> Option<Vec<u8>> {
    let pid = std::process::id();
    let threads = k.threads();
    let vsize = k.mem.size() as u64;
    let text = match canonical(path).as_str() {
        "/self/stat" => {
            let (utime, stime, minflt, majflt) = rusage();
            let h = host();
            let started = k.started_epoch.duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
            let starttime = ((started - h.boot_time as f64).max(0.0) * TICKS as f64) as u64;
            let rss = vsize / PAGE;
            format!(
                "{pid} (beam.smp) R 1 {pid} {pid} 0 -1 4194560 {minflt} 0 {majflt} 0 {utime} {stime} 0 0 20 0 {threads} 0 {starttime} {vsize} {rss} \
                 18446744073709551615 1 1 0 0 0 0 0 4096 1260 0 0 0 17 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n"
            )
        },
        "/self/statm" => {
            let pages = vsize / PAGE;
            format!("{pages} {pages} 0 0 0 {pages} 0\n")
        },
        "/self/status" => {
            let kb = vsize / 1024;
            format!("Name:\tbeam.smp\nState:\tR (running)\nPid:\t{pid}\nPPid:\t1\nThreads:\t{threads}\nVmPeak:\t{kb} kB\nVmSize:\t{kb} kB\nVmHWM:\t{kb} kB\nVmRSS:\t{kb} kB\n")
        },
        "/self/limits" => format!(
            "{:<26}{:<21}{:<21}{:<10}\n{:<26}{:<21}{:<21}{:<10}\n{:<26}{:<21}{:<21}{:<10}\n",
            "Limit", "Soft Limit", "Hard Limit", "Units", "Max open files", 4096, 4096, "files", "Max processes", "unlimited", "unlimited", "processes"
        ),
        "/meminfo" => {
            // The VM's own ceiling, as a container's cgroup limit shows: the host's memory, capped
            // at what the guest can grow to (`--memory-max`, or wasm32's 4 GiB) less what it has.
            let h = host();
            let (limit, grown) = (k.mem.limit(), k.mem.size() as u64);
            let room = limit.saturating_sub(grown);
            format!(
                "MemTotal:       {} kB\nMemFree:        {} kB\nMemAvailable:   {} kB\nBuffers:        0 kB\nCached:         0 kB\nSwapTotal:      0 kB\nSwapFree:       0 kB\n",
                h.mem_total.min(limit) / 1024,
                h.mem_free.min(room) / 1024,
                h.mem_available.min(room) / 1024
            )
        },
        "/loadavg" => {
            let l = host().loadavg;
            format!("{:.2} {:.2} {:.2} 1/{threads} {pid}\n", l[0], l[1], l[2])
        },
        "/stat" => {
            let h = host();
            let [user, nice, system, idle] = h.cpu;
            format!("cpu  {user} {nice} {system} {idle} 0 0 0 0 0 0\nbtime {}\nprocesses 1\nprocs_running 1\nprocs_blocked 0\n", h.boot_time)
        },
        "/uptime" => {
            let up = (now_secs() - host().boot_time as f64).max(0.0);
            format!("{up:.2} {up:.2}\n")
        },
        "/cpuinfo" => {
            let n = std::thread::available_parallelism().map_or(1, |n| n.get());
            (0..n).map(|i| format!("processor\t: {i}\n\n")).collect()
        },
        _ => return None,
    };
    Some(text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_treats_any_pid_as_self() {
        assert_eq!(canonical("/1234/fd"), "/self/fd");
    }

    #[test]
    fn canonical_leaves_other_paths_alone() {
        assert_eq!(canonical("/meminfo"), "/meminfo");
    }

    #[test]
    fn pid_directories_are_directories() {
        assert!(is_dir("/42"));
    }
}
