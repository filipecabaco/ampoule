//! ampoule: package an Erlang/Elixir release as one binary.
//!
//! The binary carries the BEAM compiled to wasm32-wasip1-threads (precompiled to native code)
//! and is the guest's whole operating system: wasi_snapshot_preview1, wasi.thread-spawn and the
//! ampoule_os socket module are implemented here, over one fd table shared by every wasm thread.
//!
//! Without a payload it is the packaging CLI (`release`, `pack`, `run`); `pack` appends a
//! release to a copy of itself, and that copy is the app (`start`, `serve`, `eval`, ...).

mod config;
mod engine;
mod errno;
mod hostfs;
mod kernel;
mod mem;
mod net;
mod nif;
mod pack;
mod policy;
mod proc;
mod signals;
mod threads;
mod toolchain;
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
         \n  --target T           aarch64-darwin, aarch64-linux, x86_64-linux, ... (repeatable; default: {TARGET})\
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

fn stub_for(own: &'static [u8], target: &str, a: &PackArgs) -> Result<&'static [u8]> {
    if target == TARGET && !a.stubs.contains_key(target) {
        return Ok(own);
    }
    let path = match a.stubs.get(target) {
        Some(p) => p.clone(),
        None => std::env::var_os("AMPOULE_STUBS")
            .map(|d| PathBuf::from(d).join(format!("ampoule-{target}")))
            .with_context(|| format!("no runtime for {target}: pass --stub {target}=FILE or set AMPOULE_STUBS to a directory with ampoule-{target}"))?,
    };
    let bytes: &'static [u8] = Box::leak(std::fs::read(&path).with_context(|| format!("runtime for {target}: {}", path.display()))?.into_boxed_slice());
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
            let built = nif::build(&project, &mix_env, &natives, &tc)?;
            eprintln!("==> linking the engine with {} NIF(s): {}", built.len(), built.iter().map(|b| b.libname.as_str()).collect::<Vec<_>>().join(", "));
            engine_wasm = Some(nif::link_engine(&tc, &built, &project.join("_build/ampoule-nif/engine"))?);
            manifest.nifs = built.iter().map(|b| b.libname.clone()).collect();
        }
    }
    let out_dir = match &a.out {
        Some(o) => o.clone(),
        None => release.parent().and_then(Path::parent).map(|p| p.join("ampoule")).unwrap_or_else(|| PathBuf::from("ampoule")),
    };
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
    for target in &a.targets {
        let stub = stub_for(own, target, a)?;
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
    vm_args: Option<String>,
    stop_timeout: Option<u64>,
    memory_max: Option<u64>,
    mounts: Vec<(PathBuf, String, bool)>,
    env: Vec<(String, String)>,
    tmp: Option<PathBuf>,
    hosts: Vec<(String, String)>,
    policy: policy::Policy,
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
         \n  --memory-max SIZE      AMPOULE_MEMORY_MAX   cap the VM's memory, e.g. 512M, 2G\
         \n  --schedulers N         AMPOULE_SCHEDULERS   schedulers (default: host cores)\
         \n  --vm-args FLAGS        AMPOULE_VM_ARGS      VM flags as in vm.args, e.g. \"+S :4 +SDio 2\" (after vm.args)\
         \n  --stop-timeout SECS    AMPOULE_STOP_TIMEOUT how long SIGTERM/SIGINT wait for a clean stop (default: 30)\
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
        "memory-max" => o.memory_max = Some(parse_size(v)?),
        "schedulers" => o.schedulers = Some(v.parse()?),
        "vm-args" => o.vm_args = Some(v.to_string()),
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
    ("AMPOULE_MEMORY_MAX", "memory-max"),
    ("AMPOULE_SCHEDULERS", "schedulers"),
    ("AMPOULE_VM_ARGS", "vm-args"),
    ("AMPOULE_STOP_TIMEOUT", "stop-timeout"),
    ("AMPOULE_TMP", "tmp"),
];

fn app_main(app: App, args: &[String]) -> Result<()> {
    let m = &app.manifest;
    let mut o = RunOpts::default();
    o.policy.listen = m.network.listen.clone();
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

    let eval = |expr: &str| -> Vec<String> {
        if m.elixir {
            release_args(m, "start_clean", &["-extra", "--eval", expr, "--"])
        } else {
            release_args(m, "start_clean", &["-eval", expr, "-s", "init", "stop"])
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
            let tail: &[&str] = if m.elixir { &["-mode", "embedded", "-extra", "--no-halt"] } else { &["-mode", "embedded"] };
            (release_args(m, "start", tail), m.start_env.clone())
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
    std::process::exit(run_vm(&app, &o, erl_args, defaults)?)
}

/// The erl arguments the release's own start script would pass, minus distribution (the wasm
/// runtime runs one node), plus the overlay the runtime writes to /tmp/.ampoule.
fn release_args(m: &Manifest, boot: &str, tail: &[&str]) -> Vec<String> {
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
        format!("{vsn_dir}/vm.args"),
    ]);
    a.extend(tail.iter().map(|s| s.to_string()));
    a
}

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
    Ok(())
}

fn load_engine(engine: &Engine, packed: Option<&'static [u8]>) -> Result<Module> {
    if let Some(path) = std::env::var_os("AMPOULE_ENGINE") {
        let path = PathBuf::from(path);
        return Module::from_file(engine, &path).map_err(|e| anyhow::anyhow!("compiling {}: {e:?}", path.display()));
    }
    let native = zstd::bulk::decompress(packed.unwrap_or(ENGINE), 512 << 20)?;
    // SAFETY: these bytes come from this binary (build.rs) or its own payload (nif::precompile),
    // both produced by wasmtime with engine::config() for this target; the engine checks the
    // version and configuration header before using them.
    unsafe { Module::deserialize(engine, &native) }.map_err(|e| anyhow::anyhow!("loading the engine: {e:?}"))
}

/// Allocators whose carriers come from `malloc` instead of `erl_mseg` (`-M<A>mmmbc 0 -M<A>mmsbc 0`).
/// wasi-libc's emulated `munmap` frees only the exact range `mmap` returned, but ERTS maps
/// `size + alignment` and unmaps the unaligned head and tail, then the aligned interior: those
/// calls fail, ERTS ignores the error, and every freed mseg carrier stays in linear memory, which
/// never shrinks. An idle Realtime grew by about 19 MB a minute; with this it stays flat and starts
/// about 100 MB smaller. A release's vm.args can still set these.
const CARRIERS_FROM_MALLOC: [&str; 10] = ["B", "D", "E", "F", "H", "I", "L", "R", "S", "T"];

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
        unsafe { libc::atexit(remove_owned_tmp) };
    }
    Ok(t)
}

const OWNED_TMP_PREFIX: &str = "ampoule-";
static OWNED_TMP: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
static OWNED_LOCK: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();

/// An exclusive lock on `f`, without waiting.
fn try_lock(f: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
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
/// configuration. os_mon's services all run port programs (memsup, cpu_sup, df), which the
/// runtime cannot spawn.
fn write_overlay(tmp: &Path) -> Result<()> {
    let dir = tmp.join(".ampoule");
    std::fs::write(
        dir.join("overlay.config"),
        "[{os_mon, [{start_disksup, false}, {start_memsup, false}, {start_cpu_sup, false}, {start_os_sup, false}]}].\n",
    )?;
    std::fs::write(dir.join("inetrc"), "{lookup, [file, dns]}.\n")?;
    Ok(())
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
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };
    // Before any VM thread exists, so they all inherit the mask and only the forwarder sees them.
    let signal_set = signals::block();
    let m = &app.manifest;

    let engine = Engine::new(&engine::config())?;
    let module = load_engine(&engine, app.engine)?;
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
        eprintln!("ampoule: ignoring VM flags {} (the app runs as one node)", vm.dropped.join(", "));
    }
    let mut mounts = vec![
        Mount { guest: APP.into(), backend: app_backend },
        Mount { guest: "/tmp".into(), backend: Backend::Host { path: tmp.clone(), ro: false } },
        Mount { guest: "/etc/hosts".into(), backend: Backend::Host { path: kernel::hosts_file(&tmp.join(".ampoule"), &o.hosts)?, ro: true } },
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

    // A later -S wins, so vm.args overrides the default and --schedulers overrides vm.args.
    let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
    let s = format!("{cores}:{cores}");
    let mut argv: Vec<String> = ["beam", "-S", &s, "-SDcpu", &s].map(String::from).to_vec();
    argv.extend(CARRIERS_FROM_MALLOC.iter().flat_map(|a| [format!("-M{a}mmmbc"), "0".into(), format!("-M{a}mmsbc"), "0".into()]));
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
    env.extend(o.env.iter().cloned());

    let kernel = Arc::new(Kernel::new(Mem::new(memory.clone()), argv, env.into_iter().collect(), mounts, o.policy.clone()));

    let mut linker: Linker<Ctx> = Linker::new(&engine);
    wasi::add(&mut linker)?;
    net::add(&mut linker)?;
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
    fn release_args_boot_the_release_with_the_overlay() {
        let rel = ReleaseInfo { name: "app".into(), vsn: "0.1.0".into(), erts_vsn: "16.4".into(), elixir: false };
        let a = release_args(&Manifest::new(&rel, Config::default()), "start", &[]);
        assert!(a.windows(2).any(|w| w == ["-boot", "/app/releases/0.1.0/start"]) && a.contains(&"/tmp/.ampoule/overlay".to_string()));
    }
}
