//! ampoule: package an Erlang/Elixir release as one binary.
//!
//! The binary carries the BEAM compiled to wasm32-wasip1-threads (precompiled to native code)
//! and is the guest's whole operating system: wasi_snapshot_preview1, wasi.thread-spawn and the
//! ampoule_os socket module are implemented here, over one fd table shared by every wasm thread.
//!
//! Without a payload it is the packaging CLI (`release`, `pack`, `run`); `pack` appends a
//! release to a copy of itself, and that copy is the app (`start`, `serve`, `eval`, ...).

mod cache;
mod config;
mod dist;
mod engine;
#[cfg(feature = "compiler")]
mod engine_target;
mod errno;
mod fds;
mod hostfs;
mod idle;
mod ios;
mod kernel;
mod mem;
mod native_cache;
mod net;
mod nif;
mod pack;
mod policy;
mod proc;
mod shipped;
mod signals;
mod spawn;
mod threads;
mod toolchain;
mod unix;
mod vfs;
mod vmargs;
mod wasi;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use wasmtime::{Engine, ExternType, Linker, MemoryType, Module, SharedMemory, Store};

use config::{Config, Manifest, ReleaseInfo};
use kernel::{Backend, Kernel, Mount};
use mem::Mem;

static ENGINE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/beam.cwasm.zst"));
pub const ERTS_VSN: &str = env!("AMPOULE_ERTS_VSN");
const TARGET: &str = env!("AMPOULE_TARGET");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const APP: &str = "/app";

pub fn trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("AMPOULE_TRACE").is_some())
}

pub struct Ctx {
    pub k: Arc<Kernel>,
}

/// The kernel behind a host call.
pub fn k<'a>(c: &'a wasmtime::Caller<'_, Ctx>) -> &'a Kernel {
    &c.data().k
}

/// Where the app's files come from: appended to this binary, or a release directory (`run`).
enum Image {
    Packed(&'static [u8]),
    Dir(PathBuf),
}

struct App {
    manifest: Manifest,
    image: Image,
    /// The payload's own engine (an app with NIFs), else the built-in one.
    engine: Option<&'static [u8]>,
    /// Arguments that re-invoke this app with the same options (for `serve` steps).
    reinvoke: Vec<String>,
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("ampoule: {e:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    // Rust ignores SIGPIPE, so `<binary> info | head` would panic on a closed stdout; behave like
    // other CLI tools until the VM starts (run_vm ignores it again: guest sockets get EPIPE).
    // SAFETY: setting a disposition, before any other thread exists. (rustix has no signal().)
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    // Lossy, not std::env::args(), which panics on an argument that is not UTF-8.
    let mut argv = std::env::args_os().map(|a| a.to_string_lossy().into_owned());
    let argv0 = argv.next().unwrap_or_default();
    let args: Vec<String> = argv.collect();
    match Path::new(&argv0).file_name().and_then(|n| n.to_str()) {
        Some("ampoule-cc") => return toolchain::cc_main(&args, false),
        Some("ampoule-c++") => return toolchain::cc_main(&args, true),
        _ => {},
    }
    let (stub, payload) = pack::split(pack::self_bytes()?)?;
    match payload {
        Some(p) => app_main(App { manifest: p.manifest, image: Image::Packed(p.rootfs), engine: p.engine, reinvoke: Vec::new() }, &args),
        None => cli_main(stub, &args),
    }
}

// ----------------------------------------------------------------------------------------------
// The packaging CLI

fn cli_usage() -> ! {
    eprintln!(
        "ampoule {VERSION} ({TARGET}, ERTS {ERTS_VSN}): package an Erlang/Elixir release as one binary\n\
         \nusage:\
         \n  ampoule release [pack options] [-- mix release args]   run `mix release` here, then pack it\
         \n  ampoule pack RELEASE_DIR [pack options]                 pack an existing release\
         \n  ampoule run RELEASE_DIR [--config FILE] [app options] COMMAND   run a release without packing\
         \n  ampoule version\
         \n\npack options:\
         \n  --config FILE        ampoule.toml (default: ./ampoule.toml if present)\
         \n  --out DIR            where binaries go (default: _build/<env>/ampoule)\
         \n  --target T           aarch64-darwin, aarch64-linux, x86_64-linux, ... (repeatable; default: {TARGET});\
         \n                       `ios` writes the release and liberlang.xcframework for an iPhone/iPad app instead\
         \n  --stub T=FILE        the ampoule binary to use for target T (default: $AMPOULE_STUBS/ampoule-T)\
         \n  --allow-native       drop native code (NIFs) instead of refusing to pack\
         \n  --env ENV            MIX_ENV for `release` (default: $MIX_ENV or prod)\
         \n  --name NAME          which release to build/pack when there are several\
         \n\nThe packed binary is the app: run it with `help` for its commands and options."
    );
    std::process::exit(2)
}

struct PackArgs {
    config: Option<PathBuf>,
    out: Option<PathBuf>,
    targets: Vec<String>,
    stubs: BTreeMap<String, PathBuf>,
    allow_native: bool,
    env: Option<String>,
    name: Option<String>,
    rest: Vec<String>,
    positional: Vec<String>,
}

fn parse_pack_args(args: &[String]) -> Result<PackArgs> {
    let mut a = PackArgs {
        config: None,
        out: None,
        targets: Vec::new(),
        stubs: BTreeMap::new(),
        allow_native: false,
        env: None,
        name: None,
        rest: Vec::new(),
        positional: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].clone();
        let mut val = || {
            i += 1;
            args.get(i).cloned().with_context(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--config" => a.config = Some(PathBuf::from(val()?)),
            "--out" | "-o" => a.out = Some(PathBuf::from(val()?)),
            "--target" => a.targets.push(val()?),
            "--stub" => {
                let v = val()?;
                let (t, p) = v.split_once('=').context("--stub takes TARGET=FILE")?;
                a.stubs.insert(t.to_string(), PathBuf::from(p));
            },
            "--allow-native" => a.allow_native = true,
            "--env" => a.env = Some(val()?),
            "--name" => a.name = Some(val()?),
            "-h" | "--help" => cli_usage(),
            "--" => {
                a.rest = args[i + 1..].to_vec();
                break;
            },
            s if s.starts_with('-') => bail!("unknown option {s} (see ampoule --help)"),
            _ => a.positional.push(arg),
        }
        i += 1;
    }
    if a.targets.is_empty() {
        a.targets.push(TARGET.to_string());
    }
    Ok(a)
}

fn cli_main(stub: &'static [u8], args: &[String]) -> Result<()> {
    let Some(cmd) = args.first() else { cli_usage() };
    match cmd.as_str() {
        "cc" => toolchain::cc_main(&args[1..], false),
        "c++" => toolchain::cc_main(&args[1..], true),
        "version" | "--version" => {
            println!("ampoule {VERSION} ({TARGET}, ERTS {ERTS_VSN})");
            Ok(())
        },
        "release" => {
            let a = parse_pack_args(&args[1..])?;
            let env = a.env.clone().or_else(|| std::env::var("MIX_ENV").ok()).unwrap_or_else(|| "prod".into());
            eprintln!("==> MIX_ENV={env} mix release --overwrite {}", a.rest.join(" "));
            let mut mix = std::process::Command::new("mix");
            mix.arg("release").arg("--overwrite").args(&a.rest).env("MIX_ENV", &env);
            if let Some(name) = &a.name {
                mix.arg(name);
            }
            let status = mix.status().context("running mix (is Elixir on PATH?)")?;
            if !status.success() {
                bail!("mix release failed: {status}");
            }
            let rel_root = PathBuf::from(format!("_build/{env}/rel"));
            let release = match &a.name {
                Some(n) => rel_root.join(n),
                None => only_release(&rel_root)?,
            };
            let project = std::env::current_dir()?;
            pack_release(stub, &release, &a, Some((project, env)))
        },
        "pack" => {
            let a = parse_pack_args(&args[1..])?;
            let [release] = a.positional.as_slice() else { bail!("usage: ampoule pack RELEASE_DIR [options]") };
            pack_release(stub, Path::new(release), &a, project_of(Path::new(release)))
        },
        "run" => {
            let release = args.get(1).context("usage: ampoule run RELEASE_DIR [--config FILE] [app options] COMMAND")?;
            let release = PathBuf::from(release).canonicalize().with_context(|| release.to_string())?;
            let (config_path, rest) = match args.get(2).map(String::as_str) {
                Some("--config") => (Some(PathBuf::from(args.get(3).context("--config needs a value")?)), &args[4..]),
                _ => (None, &args[2..]),
            };
            let info = ReleaseInfo::from_dir(&release)?;
            pack::check_erts(&info, ERTS_VSN)?;
            let manifest = Manifest::new(&info, load_config(config_path.as_deref())?);
            let mut reinvoke = vec!["run".to_string(), release.display().to_string()];
            if let Some(c) = &config_path {
                reinvoke.extend(["--config".to_string(), c.display().to_string()]);
            }
            app_main(App { manifest, image: Image::Dir(release), engine: None, reinvoke }, rest)
        },
        "-h" | "--help" | "help" => cli_usage(),
        other => bail!("unknown command {other} (see ampoule --help)"),
    }
}

fn only_release(rel_root: &Path) -> Result<PathBuf> {
    let dirs: Vec<PathBuf> = std::fs::read_dir(rel_root)
        .with_context(|| format!("{} not found", rel_root.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.join("releases").is_dir())
        .collect();
    match dirs.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("no release in {}", rel_root.display()),
        _ => bail!("several releases in {}: pick one with --name", rel_root.display()),
    }
}

fn load_config(path: Option<&Path>) -> Result<Config> {
    match path {
        Some(p) => Config::load(p),
        None if Path::new("ampoule.toml").exists() => Config::load(Path::new("ampoule.toml")),
        None => Ok(Config::default()),
    }
}

/// Marks this runtime's build. Packing only builds on a runtime with the same mark: one from
/// another ampoule version could not load the engine precompiled for it.
#[used]
static RUNTIME_ID: &[u8] = concat!("\0ampoule runtime ", env!("CARGO_PKG_VERSION"), " erts ", env!("AMPOULE_ERTS_VSN"), "\0").as_bytes();

fn same_runtime(stub: &[u8]) -> bool {
    stub.windows(RUNTIME_ID.len()).any(|w| w == RUNTIME_ID)
}

/// The runtime packed apps for `target` start from: `--stub`, else `ampoule-<target>` from a
/// release next to this executable (the lean runtime, without Cranelift), else `$AMPOULE_STUBS`
/// for other targets. For this machine's own target, this executable when there is none.
fn stub_for(own: &'static [u8], target: &str, a: &PackArgs) -> Result<&'static [u8]> {
    let beside = std::env::current_exe()?.parent().map(|d| d.join(format!("ampoule-{target}"))).filter(|p| p.is_file());
    let path = match (a.stubs.get(target), beside) {
        (Some(p), _) => p.clone(),
        (None, beside) if target == TARGET => match beside {
            Some(p) => p,
            None => return Ok(own),
        },
        (None, beside) => match std::env::var_os("AMPOULE_STUBS") {
            Some(d) => PathBuf::from(d).join(format!("ampoule-{target}")),
            None => beside.with_context(|| {
                format!("no runtime for {target}: put ampoule-{target} from a release next to ampoule, pass --stub {target}=FILE, or set AMPOULE_STUBS to a directory with it")
            })?,
        },
    };
    let bytes: &'static [u8] = Box::leak(std::fs::read(&path).with_context(|| format!("runtime for {target}: {}", path.display()))?.into_boxed_slice());
    if !same_runtime(bytes) {
        bail!("{} is not the runtime of this ampoule ({VERSION}, ERTS {ERTS_VSN}): use ampoule-{target} from the same release", path.display());
    }
    Ok(pack::split(bytes)?.0)
}

/// The Mix project and MIX_ENV a release directory (`<project>/_build/<env>/rel/<name>`) came from.
fn project_of(release: &Path) -> Option<(PathBuf, String)> {
    let release = release.canonicalize().ok()?;
    let env_dir = release.parent()?.parent()?;
    let project = env_dir.parent()?.parent()?;
    let env = env_dir.file_name()?.to_string_lossy().into_owned();
    project.join("mix.exs").exists().then(|| (project.to_path_buf(), env))
}

fn pack_release(own: &'static [u8], release: &Path, a: &PackArgs, project: Option<(PathBuf, String)>) -> Result<()> {
    let started = std::time::Instant::now();
    let info = ReleaseInfo::from_dir(release)?;
    let out_dir = match &a.out {
        Some(o) => o.clone(),
        None => release.parent().and_then(Path::parent).map(|p| p.join("ampoule")).unwrap_or_else(|| PathBuf::from("ampoule")),
    };

    // iOS is not a binary: the release itself, for ERTS linked into the app (see `ios`).
    if a.targets.iter().any(|t| t == ios::TARGET) {
        let out = ios::pack(release, &info, a.allow_native, &out_dir, &ios::locate()?)?;
        println!("{}", out.display());
        if a.targets.iter().all(|t| t == ios::TARGET) {
            eprintln!("==> packed in {:.1}s", started.elapsed().as_secs_f64());
            return Ok(());
        }
    }

    pack::check_erts(&info, ERTS_VSN)?;
    let mut manifest = Manifest::new(&info, load_config(a.config.as_deref())?);
    let (image, stats) = pack::build_rootfs(release)?;

    // NIFs: rebuilt for wasm and linked into an engine carried by the payload.
    let mut engine_wasm: Option<Vec<u8>> = None;
    if !stats.foreign_native.is_empty() {
        let native_list = stats.foreign_native.iter().map(String::as_str).collect::<Vec<_>>().join(", ");
        if a.allow_native {
            eprintln!("==> dropping native code (--allow-native): {native_list}");
        } else {
            let (project, mix_env) = project.with_context(|| {
                format!("the release has native code ({native_list}) to rebuild for wasm, but its Mix project was not found; run `ampoule release` in the project, or pass --allow-native")
            })?;
            let tc = toolchain::Toolchain::locate()?;
            let natives = nif::natives(&stats.foreign_native);
            let linked = (|| -> Result<_> {
                let built = nif::build(&project, &mix_env, &natives, &tc)?;
                let names = built.iter().map(|b| b.libname.as_str()).collect::<Vec<_>>().join(", ");
                let cached = cache::dir().map(|d| cache::key(&tc, &built).map(|k| (d, k))).transpose()?;
                let wasm = match cached.as_ref().and_then(|(d, k)| cache::get(d, k)) {
                    Some(wasm) => {
                        eprintln!("==> engine with {} NIF(s) from the cache: {names}", built.len());
                        wasm
                    },
                    None => {
                        eprintln!("==> linking the engine with {} NIF(s): {names}", built.len());
                        let wasm = nif::link_engine(&tc, &built, &project.join("_build/ampoule-nif/engine"))?;
                        if let Some((d, k)) = &cached {
                            cache::put(d, k, &wasm);
                        }
                        wasm
                    },
                };
                Ok((wasm, built.iter().map(|b| b.libname.clone()).collect()))
            })();
            // Whether the build worked or not: the project copy is not left under _build.
            nif::remove_project_copy(&project);
            let (wasm, nifs) = linked?;
            engine_wasm = Some(wasm);
            manifest.nifs = nifs;
        }
    }
    eprintln!(
        "==> {} {}: {} files, {:.1} MiB → {:.1} MiB image ({} paths pruned{})",
        info.name,
        info.vsn,
        stats.files,
        stats.bytes as f64 / 1_048_576.0,
        image.len() as f64 / 1_048_576.0,
        stats.dropped,
        if stats.foreign_native.is_empty() { String::new() } else { format!(", {} native libraries", stats.foreign_native.len()) }
    );
    for target in a.targets.iter().filter(|t| *t != ios::TARGET) {
        let stub = stub_for(own, target, a)?;
        // Programs in priv/ run on the host as they are: say which ones this target cannot run.
        if manifest.process.run_priv {
            for (path, program) in stats.programs.iter().filter(|(_, p)| !p.runs_on(target)) {
                eprintln!("==> warning: {} is {}; it won't run on {target}", path.trim_start_matches('/'), program.describe());
            }
        }
        let out = out_dir.join(format!("{}-{target}", info.name));
        let engine = match &engine_wasm {
            Some(wasm) => {
                eprintln!("==> precompiling the engine for {target}");
                Some(nif::precompile(wasm, target)?)
            },
            None => None,
        };
        pack::write_binary(stub, &image, engine.as_deref(), &manifest, &out)?;
        if target.ends_with("-darwin") && cfg!(target_os = "macos") {
            let _ = std::process::Command::new("codesign").args(["--force", "--sign", "-"]).arg(&out).output();
        }
        let size = std::fs::metadata(&out)?.len();
        println!("{} ({:.1} MiB)", out.display(), size as f64 / 1_048_576.0);
    }
    eprintln!("==> packed in {:.1}s; run `<binary> help` for its commands", started.elapsed().as_secs_f64());
    Ok(())
}

// ----------------------------------------------------------------------------------------------
// The app

#[derive(Default)]
struct RunOpts {
    schedulers: Option<usize>,
    /// `start`'s code loading: `embedded` (the default, as `mix release`) loads every module at
    /// boot; `interactive` loads each on first use, a smaller VM for an app that uses little of
    /// what it ships.
    mode: Option<&'static str>,
    idle_stop: Option<String>,
    restart: Option<bool>,
    memory_alarm: Option<u8>,
    /// Where the compiled engine is kept (`native_cache`); over the packed `native_cache`.
    native_cache: Option<native_cache::Setting>,
    vm_args: Option<String>,
    stop_timeout: Option<u64>,
    memory_max: Option<u64>,
    mounts: Vec<(PathBuf, String, bool)>,
    env: Vec<(String, String)>,
    tmp: Option<PathBuf>,
    hosts: Vec<(String, String)>,
    policy: policy::Policy,
    allow_exec: Vec<String>,
    dist: dist::Opts,
    /// Whether the VM runs distributed, and as what (`start`; `remote`, `rpc`).
    role: Option<dist::Role>,
}

fn app_usage(m: &Manifest) -> ! {
    let custom: String = m.commands.iter().map(|(name, expr)| format!("\n  {name:<22} {expr}")).collect();
    let serve: Vec<String> = m
        .serve
        .iter()
        .map(|s| match s {
            config::Step::Run(c) => c.clone(),
            config::Step::When { run, when } => format!("{run} (when {when})"),
        })
        .collect();
    eprintln!(
        "{} {} (ampoule {VERSION}, {TARGET})\n\
         \nusage: {} [options] <command>\n\
         \ncommands:\
         \n  start                  start the release\
         \n  serve                  {}\
         \n  eval EXPR              evaluate an expression on a non-booted release\
         \n  remote                 a shell on the running node (needs distribution)\
         \n  rpc EXPR               evaluate an expression on the running node (needs distribution)\
         \n  vm -- ERL_ARGS         run the VM with raw erl arguments (release at {APP})\
         \n  info                   what is packed{custom}\
         \n\noptions (or AMPOULE_* environment variables):\
         \n  --listen PORTS         AMPOULE_LISTEN       ports the app may bind, e.g. 4000,4001 (default: any)\
         \n  --port-map G:H,...     AMPOULE_PORT_MAP     bind app port G as host port H\
         \n  --listen-host IP       AMPOULE_LISTEN_HOST  address wildcard binds land on\
         \n  --host NAME=IP,...     AMPOULE_HOSTS        extra /etc/hosts entries\
         \n  --rewrite-connect A=B  AMPOULE_REWRITE      send connections for A (ip:port) to B\
         \n  --deny-connect ADDR    AMPOULE_DENY         refuse connections to ADDR (ip:port)\
         \n  --mount H:G[:ro],...   AMPOULE_MOUNTS       expose host paths to the app\
         \n  --allow-exec PROGS     AMPOULE_ALLOW_EXEC   host programs the app may start, e.g. git,/bin/sh (\"*\": all on PATH)\
         \n  --memory-max SIZE      AMPOULE_MEMORY_MAX   cap the VM's memory, e.g. 512M, 2G\
         \n  --schedulers N         AMPOULE_SCHEDULERS   schedulers (default: host cores)\
         \n  --mode MODE            AMPOULE_MODE         start's code loading: embedded (default) or interactive\
         \n  --vm-args FLAGS        AMPOULE_VM_ARGS      VM flags as in vm.args, e.g. \"+S :4 +SDio 2\" (after vm.args)\
         \n  --idle-stop TIME       AMPOULE_IDLE_STOP    with `start`, stop the VM after TIME without connections (300, 90s, 5m, 1h) and start it again on the next one\
         \n  --restart MODE         AMPOULE_RESTART      with `start`, on-failure: start the VM again when it fails (a crash, out of memory); off (default)\
         \n  --memory-alarm PCT     AMPOULE_MEMORY_ALARM with `start`, set the system_memory_high_watermark alarm at PCT% of the memory limit (default: 80; 0: off)\
         \n  --native-cache WHERE   AMPOULE_NATIVE_CACHE where the compiled engine is kept between starts: auto (default), off, or a directory\
         \n  --stop-timeout SECS    AMPOULE_STOP_TIMEOUT how long SIGTERM/SIGINT wait for a clean stop (default: 30)\
         \n  --node NAME            RELEASE_NODE         run `start` as a distributed node: NAME or NAME@HOST\
         \n  --distribution MODE    RELEASE_DISTRIBUTION name, sname or none (default: none, or from --node)\
         \n  --dist-port PORT       AMPOULE_DIST_PORT    the distribution port; without epmd, every node's (default: 9100)\
         \n  --epmd MODE            AMPOULE_EPMD         none: find peers on the distribution port (default); host: the host's epmd\
         \n  --cookie-file FILE     AMPOULE_COOKIE_FILE  the cookie (default: RELEASE_COOKIE, else the release's)\
         \n  --tmp DIR              AMPOULE_TMP          host directory behind /tmp\
         \n                         AMPOULE_CA_BUNDLE    CA certificates file (default: the host's)\
         \n  --env KEY=VALUE                               set an app environment variable",
        m.name,
        m.vsn,
        std::env::args_os().next().map(|a| a.to_string_lossy().into_owned()).unwrap_or_else(|| m.name.clone()),
        serve.join(", then ")
    );
    std::process::exit(2)
}

fn parse_size(s: &str) -> Result<u64> {
    let v = s.trim().to_ascii_uppercase();
    let (num, mult) = if let Some(n) = v.strip_suffix('G') {
        (n, 1u64 << 30)
    } else if let Some(n) = v.strip_suffix('M') {
        (n, 1u64 << 20)
    } else if let Some(n) = v.strip_suffix('K') {
        (n, 1u64 << 10)
    } else {
        (v.as_str(), 1)
    };
    Ok(num.parse::<u64>().with_context(|| format!("bad size {s}"))? * mult)
}

fn list(v: &str) -> impl Iterator<Item = &str> {
    v.split(',').map(str::trim).filter(|s| !s.is_empty())
}

/// Apply one option, from a flag or its AMPOULE_* variable.
fn apply_opt(o: &mut RunOpts, key: &str, v: &str) -> Result<()> {
    if o.dist.apply(key, v)? {
        return Ok(());
    }
    match key {
        "listen" => o.policy.listen = list(v).map(|p| p.parse().with_context(|| format!("bad port {p}"))).collect::<Result<_>>()?,
        "port-map" => {
            for pair in list(v) {
                let (g, h) = pair.split_once(':').context("port map entries are APP:HOST")?;
                o.policy.port_map.push((g.parse()?, h.parse()?));
            }
        },
        "listen-host" => o.policy.listen_host = Some(v.parse().with_context(|| format!("bad address {v}"))?),
        "host" => {
            for pair in list(v) {
                let (name, ip) = pair.split_once('=').context("hosts are NAME=IP")?;
                ip.parse::<std::net::IpAddr>().with_context(|| format!("bad address {ip}"))?;
                o.hosts.push((name.to_string(), ip.to_string()));
            }
        },
        "rewrite-connect" => {
            for pair in list(v) {
                let (from, to) = pair.split_once('=').context("rewrites are FROM=TO (ip:port)")?;
                o.policy.rewrite_connect.push((from.parse()?, to.parse()?));
            }
        },
        "deny-connect" => {
            for a in list(v) {
                o.policy.deny_connect.push(a.parse()?);
            }
        },
        "mount" => {
            for m in list(v) {
                let mut parts = m.splitn(3, ':');
                let host = parts.next().context("mounts are HOST:GUEST[:ro]")?;
                let guest = parts.next().context("mounts are HOST:GUEST[:ro]")?;
                let ro = matches!(parts.next(), Some("ro"));
                let host = PathBuf::from(host).canonicalize().with_context(|| format!("mount {host}"))?;
                o.mounts.push((host, kernel::normalize(guest), ro));
            }
        },
        "allow-exec" => {
            o.allow_exec = list(v).map(String::from).collect();
            config::Process::check(&o.allow_exec)?;
        },
        "memory-max" => o.memory_max = Some(parse_size(v)?),
        "schedulers" => o.schedulers = Some(v.parse()?),
        "native-cache" => o.native_cache = Some(native_cache::Setting::parse(v)?),
        "mode" => {
            o.mode = Some(match v {
                "embedded" => "embedded",
                "interactive" => "interactive",
                _ => anyhow::bail!("--mode is embedded or interactive, not {v:?}"),
            })
        },
        "vm-args" => o.vm_args = Some(v.to_string()),
        "restart" => o.restart = Some(config::parse_restart(v)?),
        "memory-alarm" => {
            let pct: u8 = v.parse().with_context(|| format!("bad percentage {v}"))?;
            if pct > 100 {
                bail!("--memory-alarm is a percentage (0-100)");
            }
            o.memory_alarm = Some(pct);
        },
        "idle-stop" => {
            idle::parse_duration(v)?;
            o.idle_stop = Some(v.to_string());
        },
        "stop-timeout" => o.stop_timeout = Some(v.parse().with_context(|| format!("bad timeout {v}"))?),
        "tmp" => o.tmp = Some(PathBuf::from(v)),
        "env" => {
            let (k, val) = v.split_once('=').context("--env takes KEY=VALUE")?;
            o.env.push((k.to_string(), val.to_string()));
        },
        _ => bail!("unknown option --{key}"),
    }
    Ok(())
}

const ENV_OPTS: &[(&str, &str)] = &[
    ("AMPOULE_LISTEN", "listen"),
    ("AMPOULE_PORT_MAP", "port-map"),
    ("AMPOULE_LISTEN_HOST", "listen-host"),
    ("AMPOULE_HOSTS", "host"),
    ("AMPOULE_REWRITE", "rewrite-connect"),
    ("AMPOULE_DENY", "deny-connect"),
    ("AMPOULE_MOUNTS", "mount"),
    ("AMPOULE_ALLOW_EXEC", "allow-exec"),
    ("AMPOULE_MEMORY_MAX", "memory-max"),
    ("AMPOULE_SCHEDULERS", "schedulers"),
    ("AMPOULE_MODE", "mode"),
    ("AMPOULE_VM_ARGS", "vm-args"),
    ("AMPOULE_IDLE_STOP", "idle-stop"),
    ("AMPOULE_RESTART", "restart"),
    ("AMPOULE_MEMORY_ALARM", "memory-alarm"),
    ("AMPOULE_NATIVE_CACHE", "native-cache"),
    ("AMPOULE_STOP_TIMEOUT", "stop-timeout"),
    ("AMPOULE_TMP", "tmp"),
    ("RELEASE_DISTRIBUTION", "distribution"),
    ("RELEASE_NODE", "node"),
    ("AMPOULE_DIST_PORT", "dist-port"),
    ("AMPOULE_EPMD", "epmd"),
    ("AMPOULE_COOKIE_FILE", "cookie-file"),
];

fn app_main(app: App, args: &[String]) -> Result<()> {
    let m = &app.manifest;
    let mut o = RunOpts::default();
    o.policy.listen = m.network.listen.clone();
    o.allow_exec = m.process.allow.clone();
    o.dist = dist::Opts::from_config(&m.distribution)?;
    for (g, h) in &m.network.port_map {
        o.policy.port_map.push((g.parse().with_context(|| format!("bad port {g} in [network] port_map"))?, *h));
    }
    for (var, key) in ENV_OPTS {
        if let Ok(v) = std::env::var(var) {
            apply_opt(&mut o, key, &v).with_context(|| var.to_string())?;
        }
    }

    let mut i = 0;
    let cmd = loop {
        let Some(arg) = args.get(i) else { app_usage(m) };
        match arg.strip_prefix("--") {
            Some("help") => app_usage(m),
            Some(key) => {
                let v = args.get(i + 1).with_context(|| format!("{arg} needs a value"))?;
                apply_opt(&mut o, key, v)?;
                i += 2;
            },
            None => break arg.as_str(),
        }
    };
    let rest = &args[i + 1..];
    o.dist.check()?;
    o.policy.exec = spawn::Exec::resolve(&o.allow_exec, &std::env::var("PATH").unwrap_or_default());

    let eval = |expr: &str| -> Vec<String> {
        if m.elixir {
            release_args(m, "start_clean", "vm.args", &["-extra", "--eval", expr, "--"])
        } else {
            release_args(m, "start_clean", "vm.args", &["-eval", expr, "-s", "init", "stop"])
        }
    };
    let (erl_args, defaults): (Vec<String>, BTreeMap<String, String>) = match cmd {
        "help" | "-h" => app_usage(m),
        "info" => return info(&app),
        "serve" => {
            let mut prefix = app.reinvoke.clone();
            prefix.extend(args[..i].iter().cloned());
            return serve(m, &prefix);
        },
        "start" => {
            o.role = Some(dist::Role::Node);
            (start_args(m, &o), m.start_env.clone())
        },
        "remote" | "rpc" => {
            let d = o
                .dist
                .resolve(&m.name)
                .with_context(|| format!("`{cmd}` connects to a distributed node: set --node or --distribution (or [distribution] in ampoule.toml)"))?;
            let expr = match cmd {
                "rpc" => Some(rest.first().context("rpc needs an expression")?.as_str()),
                _ => None,
            };
            o.role = Some(dist::Role::Client(if cmd == "rpc" { "rpc" } else { "rem" }));
            (client_args(m, &d.target(&dist::hostname()), expr), BTreeMap::new())
        },
        "eval" => (eval(rest.first().context("eval needs an expression")?), BTreeMap::new()),
        "vm" => {
            let raw: Vec<String> = rest.iter().skip_while(|a| *a == "--").cloned().collect();
            let mut a: Vec<String> = ["-kernel", "inetrc", "\"/tmp/.ampoule/inetrc\"", "-config", "/tmp/.ampoule/overlay"].map(String::from).to_vec();
            if !raw.iter().any(|x| x == "-boot") {
                a.extend([
                    "-boot".to_string(),
                    format!("{APP}/releases/{}/start_clean", m.vsn),
                    "-boot_var".into(),
                    "RELEASE_LIB".into(),
                    format!("{APP}/lib"),
                ]);
            }
            a.extend(raw);
            (a, BTreeMap::new())
        },
        custom => match m.commands.get(custom) {
            Some(expr) => (eval(expr), BTreeMap::new()),
            None => bail!("unknown command {custom} (see `help`)"),
        },
    };
    if cmd == "start" {
        let idle = match &o.idle_stop {
            Some(v) => idle::parse_duration(v)?,
            None => m.idle_stop.as_deref().map(idle::parse_duration).transpose()?.flatten(),
        };
        let restart = match o.restart {
            Some(r) => r,
            None => m.restart.as_deref().map(config::parse_restart).transpose()?.unwrap_or(false),
        };
        if idle.is_some() || restart {
            if !idle::is_child() {
                // The supervisor never starts a VM itself; each child runs this same command.
                std::process::exit(idle::supervise(idle, restart)?);
            }
            idle::init_child(idle)?;
        }
    }
    std::process::exit(run_vm(&app, &o, erl_args, defaults)?)
}

/// `start`'s erl arguments: the release booted in `--mode` (embedded by default, as `mix release`),
/// with the memory alarm unless it is off.
fn start_args(m: &Manifest, o: &RunOpts) -> Vec<String> {
    let mode = o.mode.unwrap_or("embedded");
    let tail: &[&str] = if m.elixir { &["-mode", mode, "-extra", "--no-halt"] } else { &["-mode", mode] };
    let mut a = release_args(m, "start", "vm.args", tail);
    let pct = o.memory_alarm.or(m.memory_alarm).unwrap_or(config::MEMORY_ALARM);
    if pct > 0 {
        // An init flag, so before -extra; init evaluates it once the release has booted.
        let at = a.iter().position(|x| x == "-extra").unwrap_or(a.len());
        a.splice(at..at, ["-eval".to_string(), config::memory_alarm_eval(pct)]);
    }
    a
}

/// The erl arguments the release's own start script would pass, plus the overlay the runtime
/// writes to /tmp/.ampoule. Distribution is added by `run_vm` (see `dist`); `vm_args` is the
/// release's args file (`vm.args`, or `remote.vm.args` for `remote` and `rpc`).
fn release_args(m: &Manifest, boot: &str, vm_args: &str, tail: &[&str]) -> Vec<String> {
    let vsn_dir = format!("{APP}/releases/{}", m.vsn);
    let mut a: Vec<String> = vec!["-noshell".into()];
    if m.elixir {
        a.extend(["-s", "elixir", "start_cli"].map(String::from));
    }
    a.extend([
        "-kernel".into(),
        "inetrc".into(),
        "\"/tmp/.ampoule/inetrc\"".into(),
        "-config".into(),
        format!("{vsn_dir}/sys"),
        "-config".into(),
        "/tmp/.ampoule/overlay".into(),
        "-boot".into(),
        format!("{vsn_dir}/{boot}"),
        "-boot_var".into(),
        "RELEASE_LIB".into(),
        format!("{APP}/lib"),
        "-args_file".into(),
        format!("{vsn_dir}/{vm_args}"),
    ]);
    a.extend(tail.iter().map(|s| s.to_string()));
    a
}

/// `remote` (`expr` None) and `rpc`, as the release's own script runs them: a shell attached to
/// `target` (IEx's `--remsh`, erl's `-remsh`), or one expression evaluated there and printed.
fn client_args(m: &Manifest, target: &str, expr: Option<&str>) -> Vec<String> {
    let mut a = release_args(m, "start_clean", "remote.vm.args", &[]);
    match (expr, m.elixir) {
        (Some(expr), true) => a.extend(["-extra", "--rpc-eval", target, expr].map(String::from)),
        (Some(expr), false) => {
            // Plain arguments rather than a string in the code, so the expression needs no quoting.
            a.extend(["-eval".to_string(), ERL_RPC_EVAL.to_string(), "-s".into(), "init".into(), "stop".into(), "-extra".into(), target.into(), expr.into()])
        },
        (None, true) => {
            // iex runs `elixir --erl "-user elixir" +iex`: IEx starts as the user process instead of
            // `elixir start_cli`.
            if let Some(at) = a.windows(3).position(|w| w == ["-s", "elixir", "start_cli"]) {
                a.splice(at..at + 3, ["-user".to_string(), "elixir".to_string()]);
            }
            a.extend(["-extra", "--no-halt", "+iex", "--remsh", target].map(String::from));
        },
        (None, false) => {
            a.retain(|x| x != "-noshell");
            a.extend(["-remsh".to_string(), target.to_string()]);
        },
    }
    a
}

/// `rpc` for a release without Elixir: the node and the expression come as plain arguments.
const ERL_RPC_EVAL: &str = r#"[Node, Expr] = init:get_plain_arguments(),
Src = case lists:reverse(string:trim(Expr)) of [$. | _] -> Expr; _ -> Expr ++ "." end,
{ok, Tokens, _} = erl_scan:string(Src),
{ok, Exprs} = erl_parse:parse_exprs(Tokens),
{value, Value, _} = erpc:call(list_to_atom(Node), erl_eval, exprs, [Exprs, []]),
io:format("~p~n", [Value])."#;

/// Run each serve step as its own VM (a guest exit ends the whole runtime process); the last
/// step replaces this process.
fn serve(m: &Manifest, prefix: &[String]) -> Result<()> {
    let me = std::env::current_exe()?;
    let steps: Vec<&str> = m.serve.iter().filter_map(|s| s.command()).collect();
    let Some((last, before)) = steps.split_last() else { bail!("nothing to serve") };
    for step in before {
        eprintln!("==> {step}");
        let status = std::process::Command::new(&me).args(prefix).arg(step).status()?;
        if !status.success() {
            bail!("{step} failed: {status}");
        }
    }
    eprintln!("==> {last}");
    use std::os::unix::process::CommandExt;
    Err(std::process::Command::new(&me).args(prefix).arg(last).exec().into())
}

fn info(app: &App) -> Result<()> {
    let m = &app.manifest;
    println!("app:       {} {}{}", m.name, m.vsn, if m.elixir { " (Elixir)" } else { "" });
    match &app.image {
        Image::Packed(bytes) => {
            let fs = vfs::RootFs::parse(bytes)?;
            println!("files:     {} ({:.1} MiB image)", fs.files(), bytes.len() as f64 / 1_048_576.0);
        },
        Image::Dir(d) => println!("files:     {} (not packed)", d.display()),
    }
    println!("runtime:   ampoule {VERSION} for {TARGET}, ERTS {ERTS_VSN} (release built for {})", m.erts_vsn);
    if !m.commands.is_empty() {
        println!("commands:  {}", m.commands.keys().cloned().collect::<Vec<_>>().join(", "));
    }
    if !m.nifs.is_empty() {
        println!("nifs:      {} (linked into the packed engine)", m.nifs.join(", "));
    }
    let listen = if m.network.listen.is_empty() { "any port".to_string() } else { format!("{:?}", m.network.listen) };
    println!("network:   listen {listen}");
    if !m.process.allow.is_empty() {
        println!("processes: {}", m.process.allow.join(", "));
    }
    if let Some(d) = dist::Opts::from_config(&m.distribution)?.resolve(&m.name) {
        let port = match (d.epmd, d.listen_port()) {
            (dist::Epmd::None, Some(p)) => format!("port {p}, no epmd"),
            (_, Some(p)) => format!("port {p}, the host's epmd"),
            (_, None) => "the host's epmd".into(),
        };
        println!("dist:      {} {} ({port})", if d.long { "-name" } else { "-sname" }, d.node);
    }
    Ok(())
}

/// The engine: `AMPOULE_ENGINE`, else the payload's or the built-in one. `writable` are the host
/// paths the app can write, where the native engine cache must not be.
fn load_engine(engine: &Engine, packed: Option<&'static [u8]>, cache: &native_cache::Setting, writable: &[&Path]) -> Result<Module> {
    if let Some(path) = std::env::var_os("AMPOULE_ENGINE") {
        let path = PathBuf::from(path);
        #[cfg(feature = "compiler")]
        return Module::from_file(engine, &path).map_err(|e| anyhow::anyhow!("compiling {}: {e:?}", path.display()));
        #[cfg(not(feature = "compiler"))]
        anyhow::bail!("AMPOULE_ENGINE={}: this ampoule runs only precompiled engines (built without the `compiler` feature)", path.display());
    }
    let (digest, compressed) = native_cache::split(packed.unwrap_or(ENGINE)).context("the engine is damaged")?;
    // Mapped from a file the first start writes (native_cache): clean pages the kernel can reclaim,
    // instead of a private copy of the whole engine.
    let mut decompressed = None;
    if let Some(dir) = native_cache::dir(cache, writable) {
        match native_cache::load(engine, digest, compressed, &dir) {
            Ok(module) => return Ok(module),
            Err(native) => decompressed = native,
        }
    }
    let native = match decompressed {
        Some(native) => native,
        None => zstd::bulk::decompress(compressed, 512 << 20)?,
    };
    // SAFETY: these bytes come from this binary (build.rs) or its own payload (nif::precompile),
    // both produced by wasmtime with engine::config() for this target; the engine checks the
    // version and configuration header before using them.
    unsafe { Module::deserialize(engine, &native) }.map_err(|e| anyhow::anyhow!("loading the engine: {e:?}"))
}

/// ERTS keeps up to 10 freed memory segments per scheduler for reuse (`+MMmcs`). Each is held by
/// one allocator instance, and with the memory capped (`--memory-max`, or wasm32's 4 GiB) they
/// make allocations fail that the free memory would serve: a 1 GiB spike repeated under a 1.6 GiB
/// cap ran out of memory with them and not without. The engine's segment allocator keeps up to
/// 32 MiB of freed pages warm instead, shared and always reusable (engine/ampoule-os/src/aos.c).
/// A release's vm.args can still set it.
const NO_SEGMENT_CACHE: [&str; 2] = ["-MMmcs", "0"];

/// A file of the release by its guest path (an `-args_file`), or None if the release has no such
/// file. Only files inside the release can be read before the VM starts.
fn read_app_file(app: &Backend, guest: &str) -> Result<Option<String>> {
    let normalized = kernel::normalize(guest);
    let Some(rel) = normalized.strip_prefix(APP).filter(|r| r.starts_with('/')) else {
        bail!("-args_file {guest}: only files inside the release ({APP}) can be read");
    };
    let bytes = match app {
        Backend::Embedded(fs) => match fs.lookup(rel, true) {
            Some((_, node)) if node.kind == vfs::KIND_FILE => fs.read(node)?,
            _ => return Ok(None),
        },
        Backend::Host { path, .. } => match std::fs::read(path.join(rel.trim_start_matches('/'))) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {guest}")),
        },
        Backend::Proc => return Ok(None),
    };
    Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
}

/// The host directory behind the guest's /tmp: `--tmp`, kept as is, or a per-process directory
/// the runtime owns and removes when it exits.
fn private_tmp(o: &RunOpts) -> Result<PathBuf> {
    if let Some(t) = &o.tmp {
        std::fs::create_dir_all(t.join(".ampoule"))?;
        return Ok(t.canonicalize()?);
    }
    let base = std::env::temp_dir();
    sweep_stale_tmps(&base);
    // Named after the pid, but owned by whoever holds its lock: two runtimes with the same pid
    // (in different PID namespaces sharing a /tmp) get different directories.
    let pid = std::process::id();
    let (t, lock) = (0..)
        .map(|n| base.join(if n == 0 { format!("{OWNED_TMP_PREFIX}{pid}") } else { format!("{OWNED_TMP_PREFIX}{pid}-{n}") }))
        .take(64)
        .find_map(|t| {
            std::fs::create_dir_all(t.join(".ampoule")).ok()?;
            let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(t.join(".ampoule/lock")).ok()?;
            try_lock(&lock).then_some((t, lock))
        })
        .context("no private /tmp directory available")?;
    let t = t.canonicalize()?;
    // Held, never released, for the life of the process: the kernel drops it however it ends.
    let _ = OWNED_LOCK.set(lock);
    if OWNED_TMP.set(t.clone()).is_ok() {
        // Every way the guest ends the process (proc_exit, a trap, a raised signal) goes
        // through exit(3), so this runs whichever thread ends it.
        // SAFETY: registers a plain `extern "C" fn` that only touches statics set before it.
        unsafe { libc::atexit(remove_owned_tmp) };
    }
    Ok(t)
}

const OWNED_TMP_PREFIX: &str = "ampoule-";
static OWNED_TMP: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
static OWNED_LOCK: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();

/// An exclusive lock on `f`, without waiting.
fn try_lock(f: &std::fs::File) -> bool {
    f.try_lock().is_ok()
}

extern "C" fn remove_owned_tmp() {
    if let Some(t) = OWNED_TMP.get() {
        if let Some(dump) = remove_unless_crashed(t) {
            eprintln!("ampoule: crash dump kept at {}", dump.display());
        }
    }
}

/// Remove a private /tmp unless the VM left a crash dump in it; returns the dump's path then.
fn remove_unless_crashed(t: &Path) -> Option<PathBuf> {
    let dump = t.join("erl_crash.dump");
    if dump.exists() {
        return Some(dump);
    }
    let _ = std::fs::remove_dir_all(t);
    None
}

/// Private /tmp directories of runtimes that died without exiting (killed by a signal): those
/// whose lock nobody holds. The lock, not the pid, decides, because pids are per PID namespace.
fn sweep_stale_tmps(base: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let ours = name.to_str().and_then(|n| n.strip_prefix(OWNED_TMP_PREFIX)).is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
        if !ours {
            continue;
        }
        let dir = e.path();
        let Ok(lock) = std::fs::OpenOptions::new().write(true).open(dir.join(".ampoule/lock")) else { continue };
        if try_lock(&lock) {
            remove_unless_crashed(&dir);
        }
    }
}

/// Files the runtime adds next to the release: sys.config overrides and the resolver
/// configuration. os_mon's services all run port programs (memsup, cpu_sup, df), which would run
/// on the host and report on it rather than on the VM, when the host can run them at all.
fn write_overlay(tmp: &Path) -> Result<()> {
    let dir = tmp.join(".ampoule");
    std::fs::write(
        dir.join("overlay.config"),
        "[{os_mon, [{start_disksup, false}, {start_memsup, false}, {start_cpu_sup, false}, {start_os_sup, false}]}].\n",
    )?;
    std::fs::write(dir.join("inetrc"), "{lookup, [file, dns]}.\n")?;
    Ok(())
}

/// Distribution for the command (see `dist`): its init arguments go into `vm` before `-extra`, the
/// distribution port into the policy's `listen`, and the host's own name into `hosts`. Returns
/// the environment the node sees. Without distribution, vm.args' distribution flags are left out.
fn distribution(
    o: &RunOpts,
    m: &Manifest,
    app: &Backend,
    vm: &mut vmargs::Expanded,
    policy: &mut policy::Policy,
    hosts: &mut Vec<(String, String)>,
) -> Result<Vec<(String, String)>> {
    let (role, mut d) = match (o.role, o.dist.resolve(&m.name)) {
        (Some(role), Some(d)) => (role, d),
        // Configured, but this command runs on its own (eval, a custom command, vm).
        (None, Some(_)) => return Ok(Vec::new()),
        (_, None) => {
            if !vm.dist.is_empty() {
                let flags: Vec<String> = vm.dist.iter().map(|(f, v)| dist::shown(f, v.as_deref())).collect();
                eprintln!("ampoule: ignoring VM flags {} (distribution is off; see --node)", flags.join(", "));
            }
            return Ok(Vec::new());
        },
    };
    let from_vm = dist::from_vm_args(&vm.dist);
    if d.port.is_none() && d.epmd == dist::Epmd::None {
        d.port = from_vm.port;
    }
    if !from_vm.ignored.is_empty() {
        eprintln!("ampoule: ignoring VM flags {} (the node is set by the distribution options)", from_vm.ignored.join(", "));
    }
    let release_cookie = read_app_file(app, &format!("{APP}/releases/COOKIE"))?;
    let (cookie, source) = d.cookie(std::env::var("RELEASE_COOKIE").ok(), release_cookie)?;
    if trace() {
        eprintln!("ampoule: distribution as {} with the cookie from {source}", d.node);
    }
    let mut args = d.args(role, &cookie, std::env::var("ERL_EPMD_PORT").ok().as_deref());
    args.extend(from_vm.keep);
    let at = vm.erl.iter().position(|a| a == "-extra").unwrap_or(vm.erl.len());
    vm.erl.splice(at..at, args);

    if let (dist::Role::Node, Some(port)) = (role, d.listen_port()) {
        if !policy.listen.is_empty() && !policy.listen.contains(&port) {
            policy.listen.push(port);
        }
        if d.epmd == dist::Epmd::Host && policy.port_map.iter().any(|(g, _)| *g == port) {
            bail!("--port-map cannot map the distribution port {port} with epmd = host: epmd would give peers the app's port");
        }
    }
    let known = std::fs::read_to_string("/etc/hosts").unwrap_or_default() + &hosts.iter().map(|(name, ip)| format!("{ip} {name}\n")).collect::<String>();
    hosts.extend(dist::self_hosts(&dist::hostname(), &known));
    Ok(match role {
        dist::Role::Node => vec![("RELEASE_NODE".into(), d.node.clone()), ("RELEASE_DISTRIBUTION".into(), if d.long { "name" } else { "sname" }.into())],
        dist::Role::Client(_) => Vec::new(),
    })
}

/// The host's CA certificates (AMPOULE_CA_BUNDLE, or the usual Linux/macOS locations), served
/// where OTP's public_key looks on Linux.
fn ca_bundle() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("AMPOULE_CA_BUNDLE") {
        return Some(PathBuf::from(p));
    }
    ["/etc/ssl/certs/ca-certificates.crt", "/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/ca-bundle.pem", "/etc/ssl/cert.pem"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
}

fn run_vm(app: &App, o: &RunOpts, erl_args: Vec<String>, defaults: BTreeMap<String, String>) -> Result<i32> {
    // SAFETY: setting a disposition, before the VM's threads exist.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    // Before any VM thread exists, so they all inherit the mask and only the forwarder sees them.
    let signal_set = signals::block();
    let m = &app.manifest;

    let engine = Engine::new(&engine::config())?;
    let writable: Vec<&Path> = o.mounts.iter().filter(|(_, _, ro)| !ro).map(|(host, _, _)| host.as_path()).chain(o.tmp.as_deref()).collect();
    // The flag or variable, else what the app was packed with, else auto.
    let cache = match &o.native_cache {
        Some(setting) => setting.clone(),
        None => m.native_cache.as_deref().map(native_cache::Setting::parse).transpose()?.unwrap_or_default(),
    };
    let module = load_engine(&engine, app.engine, &cache, &writable)?;
    let mut mem_ty = module
        .imports()
        .find_map(|i| match (i.module(), i.name(), i.ty()) {
            ("env", "memory", ExternType::Memory(t)) => Some(t),
            _ => None,
        })
        .context("beam.wasm does not import env.memory")?;
    if let Some(max) = o.memory_max {
        let declared = mem_ty.maximum().unwrap_or(65536);
        let pages = (max / 65536).clamp(mem_ty.minimum(), declared);
        mem_ty = MemoryType::shared(u32::try_from(mem_ty.minimum())?, u32::try_from(pages)?);
    }
    let memory = SharedMemory::new(&engine, mem_ty)?;

    let tmp = private_tmp(o)?;
    write_overlay(&tmp)?;
    let app_backend = match &app.image {
        Image::Packed(bytes) => Backend::Embedded(Arc::new(vfs::RootFs::parse(bytes)?)),
        Image::Dir(dir) => Backend::Host { path: dir.clone(), ro: true },
    };
    let mut read = |file: &str| read_app_file(&app_backend, file);
    let mut vm = vmargs::expand(&erl_args, &mut read)?;
    // ampoule.toml's vm_args, then --vm-args, after the release's vm.args: later flags win. Their
    // init arguments go before -extra, where the files' arguments already are.
    let before = vm.erl.len();
    vmargs::add("vm_args in ampoule.toml", &m.vm_args, &mut read, &mut vm)?;
    if let Some(text) = &o.vm_args {
        vmargs::add("--vm-args", text, &mut read, &mut vm)?;
    }
    let added = vm.erl.split_off(before);
    let at = vm.erl.iter().position(|a| a == "-extra").unwrap_or(vm.erl.len());
    vm.erl.splice(at..at, added);
    if !vm.dropped.is_empty() {
        eprintln!("ampoule: ignoring VM flags {}", vm.dropped.join(", "));
    }
    // Programs from the release's priv/ are extracted below /tmp/.ampoule (see `shipped`), fresh
    // for each run.
    let mut policy = o.policy.clone();
    if m.process.run_priv {
        let dir = tmp.join(".ampoule/priv");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
        policy.exec.priv_dir = Some(dir);
    }
    let mut hosts = o.hosts.clone();
    let dist_env = distribution(o, m, &app_backend, &mut vm, &mut policy, &mut hosts)?;
    let mut mounts = vec![
        Mount { guest: APP.into(), backend: app_backend },
        Mount { guest: "/tmp".into(), backend: Backend::Host { path: tmp.clone(), ro: false } },
        // The runtime's own files, read-only to the app: the overlay, /etc/hosts' source, the
        // extracted priv/ programs it runs.
        Mount { guest: "/tmp/.ampoule".into(), backend: Backend::Host { path: tmp.join(".ampoule"), ro: true } },
        Mount { guest: "/etc/hosts".into(), backend: Backend::Host { path: kernel::hosts_file(&tmp.join(".ampoule"), &hosts)?, ro: true } },
        Mount { guest: "/dev/null".into(), backend: Backend::Host { path: "/dev/null".into(), ro: false } },
        Mount { guest: "/proc".into(), backend: Backend::Proc },
    ];
    if Path::new("/etc/resolv.conf").exists() {
        mounts.push(Mount { guest: "/etc/resolv.conf".into(), backend: Backend::Host { path: "/etc/resolv.conf".into(), ro: true } });
    }
    if let Some(bundle) = ca_bundle() {
        mounts.push(Mount { guest: "/etc/ssl/certs/ca-certificates.crt".into(), backend: Backend::Host { path: bundle, ro: true } });
    }
    for (host, guest, ro) in &o.mounts {
        mounts.push(Mount { guest: guest.clone(), backend: Backend::Host { path: host.clone(), ro: *ro } });
    }
    // The programs the app may start, where os:find_executable looks for them.
    mounts.extend(o.policy.exec.mounts(&[APP, "/proc", "/tmp"]));

    // A later -S wins, so vm.args overrides the default and --schedulers overrides vm.args.
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let s = format!("{cores}:{cores}");
    let mut argv: Vec<String> = ["beam", "-S", &s, "-SDcpu", &s].map(String::from).to_vec();
    argv.extend(NO_SEGMENT_CACHE.map(String::from));
    vmargs::resolve_schedulers(&mut vm.emu, cores);
    argv.extend(vm.emu);
    if let Some(n) = o.schedulers {
        argv.extend(["-S".into(), format!("{n}:{n}"), "-SDcpu".into(), format!("{n}:{n}")]);
    }
    let bindir = format!("{APP}/erts-{}/bin", m.erts_vsn);
    argv.extend(["--", "-root", APP, "-bindir", &bindir, "-progname", "erl", "--", "-home", "/tmp", "--"].map(String::from));
    argv.extend(vm.erl);
    if !vm.extra.is_empty() {
        if !argv.iter().any(|a| a == "-extra") {
            argv.push("-extra".into());
        }
        argv.extend(vm.extra);
    }

    let reserved = |k: &str| {
        k.starts_with("RELEASE_") || k.starts_with("ERL_") || k.starts_with("AMPOULE_") || matches!(k, "ROOTDIR" | "BINDIR" | "EMU" | "PROGNAME" | "HOME")
    };
    let mut env: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in m.env.iter().chain(defaults.iter()) {
        env.insert(k.clone(), v.clone());
    }
    // vars_os: std::env::vars() panics on a variable that is not UTF-8; such variables are skipped.
    env.extend(std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).filter(|(k, _)| !reserved(k)));
    // vm.args -env overrides the inherited environment, as erlexec does.
    env.extend(vm.env);
    for (k, v) in [
        ("ROOTDIR", APP.to_string()),
        ("BINDIR", bindir.clone()),
        ("EMU", "beam".into()),
        ("PROGNAME", "erl".into()),
        ("HOME", "/tmp".into()),
        ("RELEASE_ROOT", APP.into()),
        ("RELEASE_NAME", m.name.clone()),
        ("RELEASE_VSN", m.vsn.clone()),
        ("RELEASE_SYS_CONFIG", format!("{APP}/releases/{}/sys", m.vsn)),
        ("RELEASE_TMP", "/tmp".into()),
        ("ERL_CRASH_DUMP", "/tmp/erl_crash.dump".into()),
        ("ERL_CRASH_DUMP_SECONDS", "10".into()),
    ] {
        env.insert(k.to_string(), v);
    }
    env.extend(dist_env);
    env.extend(o.env.iter().cloned());

    let kernel = Arc::new(Kernel::new(Mem::new(memory.clone()), argv, env.into_iter().collect(), mounts, policy));

    let mut linker: Linker<Ctx> = Linker::new(&engine);
    wasi::add(&mut linker)?;
    net::add(&mut linker)?;
    spawn::add(&mut linker)?;
    wasi::add_ampoule_os(&mut linker)?;
    mem::add(&mut linker)?;
    threads::add(&mut linker)?;
    let store = Store::new(&engine, Ctx { k: kernel.clone() });
    linker.define(&store, "env", "memory", memory)?;
    drop(store);

    let spawner = threads::Spawner { engine, module, linker: Arc::new(linker), kernel };
    if threads::SPAWNER.set(spawner).is_err() {
        bail!("runtime started twice");
    }
    let spawner = threads::SPAWNER.get().context("runtime not started")?;
    signals::spawn(spawner.kernel.clone(), signal_set, std::time::Duration::from_secs(o.stop_timeout.unwrap_or(30)))?;
    idle::watch(spawner.kernel.clone(), std::time::Duration::from_secs(o.stop_timeout.unwrap_or(30)))?;
    Ok(threads::run_main(spawner))
}

/// A scratch directory for tests, removed when dropped.
#[cfg(test)]
pub struct TestDir(pub PathBuf);

#[cfg(test)]
impl TestDir {
    pub fn new(name: &str) -> TestDir {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ampoule-test-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TestDir(dir)
    }
}

#[cfg(test)]
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runtime_is_recognised_by_its_mark() {
        let mut stub = b"\x7fELF some code".to_vec();
        assert!(!same_runtime(&stub));
        stub.extend_from_slice(RUNTIME_ID);
        stub.extend_from_slice(b"more");
        assert!(same_runtime(&stub));
        // Another version's mark is not this one.
        let other = String::from_utf8_lossy(RUNTIME_ID).replace(VERSION, "0.0.0");
        assert!(!same_runtime(other.as_bytes()));
    }

    #[test]
    fn parse_size_understands_suffixes() {
        assert_eq!((parse_size("512M").unwrap(), parse_size("2g").unwrap(), parse_size("4096").unwrap()), (512 << 20, 2 << 30, 4096));
    }

    #[test]
    fn parse_size_rejects_garbage() {
        assert!(parse_size("lots").is_err());
    }

    #[test]
    fn apply_opt_parses_mount_with_read_only_flag() {
        let mut o = RunOpts::default();
        apply_opt(&mut o, "mount", "/:/host/../data:ro").unwrap();
        assert_eq!(o.mounts, vec![(PathBuf::from("/"), "/data".to_string(), true)]);
    }

    #[test]
    fn apply_opt_rejects_unknown_options() {
        assert!(apply_opt(&mut RunOpts::default(), "nope", "1").is_err());
    }

    #[test]
    fn apply_opt_takes_a_native_cache_setting() {
        let mut o = RunOpts::default();
        apply_opt(&mut o, "native-cache", "off").unwrap();
        assert_eq!(o.native_cache, Some(native_cache::Setting::Off));
        assert!(apply_opt(&mut o, "native-cache", "here").is_err());
    }

    #[test]
    fn apply_opt_takes_embedded_or_interactive_mode() {
        let mut o = RunOpts::default();
        apply_opt(&mut o, "mode", "interactive").unwrap();
        assert_eq!(o.mode, Some("interactive"));
        apply_opt(&mut o, "mode", "embedded").unwrap();
        assert_eq!(o.mode, Some("embedded"));
        assert!(apply_opt(&mut o, "mode", "Interactive").is_err() && apply_opt(&mut o, "mode", "").is_err());
    }

    #[test]
    fn start_boots_in_the_mode_asked_for() {
        let rel = ReleaseInfo { name: "app".into(), vsn: "0.1.0".into(), erts_vsn: "16.4".into(), elixir: true };
        let m = Manifest::new(&rel, Config::default());
        let mode = |a: &[String]| a.windows(2).find(|w| w[0] == "-mode").map(|w| w[1].clone());
        assert_eq!(mode(&start_args(&m, &RunOpts::default())).as_deref(), Some("embedded"));
        let interactive = start_args(&m, &RunOpts { mode: Some("interactive"), ..RunOpts::default() });
        assert_eq!(mode(&interactive).as_deref(), Some("interactive"));
        // Still init flags, before -extra.
        let at = |x: &str| interactive.iter().position(|a| a == x).unwrap();
        assert!(at("-mode") < at("-extra") && at("-eval") < at("-extra"));
    }

    #[test]
    fn release_args_boot_the_release_with_the_overlay() {
        let rel = ReleaseInfo { name: "app".into(), vsn: "0.1.0".into(), erts_vsn: "16.4".into(), elixir: false };
        let a = release_args(&Manifest::new(&rel, Config::default()), "start", "vm.args", &[]);
        assert!(a.windows(2).any(|w| w == ["-boot", "/app/releases/0.1.0/start"]) && a.contains(&"/tmp/.ampoule/overlay".to_string()));
    }

    #[test]
    fn client_args_run_rpc_and_remote_as_the_release_scripts_do() {
        let rel = ReleaseInfo { name: "app".into(), vsn: "0.1.0".into(), erts_vsn: "16.4".into(), elixir: true };
        let m = Manifest::new(&rel, Config::default());
        let rpc = client_args(&m, "app@h", Some("IO.puts(1)"));
        assert!(rpc.ends_with(&["-extra", "--rpc-eval", "app@h", "IO.puts(1)"].map(String::from)));
        assert!(rpc.windows(3).any(|w| w == ["-s", "elixir", "start_cli"]) && rpc.contains(&"/app/releases/0.1.0/remote.vm.args".to_string()));
        let remote = client_args(&m, "app@h", None);
        assert!(remote.ends_with(&["-extra", "--no-halt", "+iex", "--remsh", "app@h"].map(String::from)));
        assert!(remote.windows(2).any(|w| w == ["-user", "elixir"]) && !remote.contains(&"start_cli".to_string()));
        let erl = Manifest::new(&ReleaseInfo { elixir: false, ..rel }, Config::default());
        let remote = client_args(&erl, "app@h", None);
        assert!(remote.ends_with(&["-remsh", "app@h"].map(String::from)) && !remote.contains(&"-noshell".to_string()));
        assert!(client_args(&erl, "app@h", Some("1 + 1")).ends_with(&["-extra", "app@h", "1 + 1"].map(String::from)));
    }
}
