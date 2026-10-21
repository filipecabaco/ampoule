//! What a packed binary knows about its app: the manifest, built at pack time from the release
//! itself and an optional config file (`ampoule.toml`):
//!
//! ```toml
//! serve = ["migrate", { run = "seed", when = "SEED=true" }, "start"]   # `<binary> serve`
//! vm_args = "+S :4 +SDio 2"        # VM flags as in vm.args, after the release's own
//! idle_stop = "5m"               # `start` stops the VM after this long without connections
//! restart = "on-failure"         # `start` starts the VM again when it fails (crash, out of memory)
//! memory_alarm = 80              # set system_memory_high_watermark at this % of the VM's memory limit
//!
//! [commands]                       # extra commands: `<binary> migrate` evaluates the expression
//! migrate = "MyApp.Release.migrate()"
//! seed = "MyApp.Release.seed()"
//!
//! [env]                            # defaults for every command; the real environment wins
//! LANG = "C.UTF-8"
//!
//! [start.env]                      # defaults for `start` only
//! PHX_SERVER = "true"
//!
//! [network]
//! listen = [4000]                  # the only ports the app may bind (default: any)
//! port_map = { 4000 = 8080 }       # app port -> host port
//!
//! [process]
//! allow = ["git", "/bin/sh"]       # host programs the app may start ("*": everything on PATH)
//! priv = false                     # programs in the release's own priv/ may not run (default: they may)
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Step {
    Run(String),
    When { run: String, when: String },
}

impl Step {
    /// The command to run, or None when its condition does not hold. `when` is `VAR` (set and
    /// non-empty) or `VAR=value`.
    pub fn command(&self) -> Option<&str> {
        match self {
            Step::Run(c) => Some(c),
            Step::When { run, when } => {
                let holds = match when.split_once('=') {
                    Some((var, want)) => std::env::var(var).as_deref() == Ok(want),
                    None => std::env::var(when).map(|v| !v.is_empty()).unwrap_or(false),
                };
                holds.then_some(run.as_str())
            },
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    #[serde(default)]
    pub listen: Vec<u16>,
    #[serde(default)]
    pub port_map: BTreeMap<String, u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Process {
    /// Host programs the app may start: bare names (looked up on PATH), absolute paths, or `*`.
    #[serde(default)]
    pub allow: Vec<String>,
    /// `priv`: programs in the release's own priv/ directories may run (the default).
    #[serde(rename = "priv", default = "yes")]
    pub run_priv: bool,
}

impl Default for Process {
    fn default() -> Process {
        Process { allow: Vec::new(), run_priv: true }
    }
}

fn yes() -> bool {
    true
}

impl Process {
    pub fn check(allow: &[String]) -> Result<()> {
        for entry in allow {
            if entry != "*" && !entry.starts_with('/') && entry.contains('/') {
                bail!("`{entry}`: allow takes program names, absolute paths or \"*\"");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartConfig {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub commands: BTreeMap<String, String>,
    #[serde(default)]
    pub serve: Vec<Step>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub start: StartConfig,
    #[serde(default)]
    pub network: Network,
    #[serde(default)]
    pub process: Process,
    /// VM flags written as in vm.args, applied after the release's vm.args.
    #[serde(default)]
    pub vm_args: String,
    /// How long `start` runs without connections before stopping the VM (see `idle`).
    #[serde(default)]
    pub idle_stop: Option<String>,
    /// "on-failure": `start` restarts a VM that fails (see `idle`); "off" (the default) does not.
    #[serde(default)]
    pub restart: Option<String>,
    /// The share of the memory limit, in percent, at which `start` raises the memory alarm (0: never).
    #[serde(default)]
    pub memory_alarm: Option<u8>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        for reserved in BUILTIN_COMMANDS {
            if cfg.commands.contains_key(*reserved) {
                bail!("{}: [commands] cannot redefine the built-in `{reserved}`", path.display());
            }
        }
        Process::check(&cfg.process.allow).with_context(|| format!("{}: [process]", path.display()))?;
        if let Some(d) = &cfg.idle_stop {
            crate::idle::parse_duration(d).with_context(|| format!("{}: idle_stop", path.display()))?;
        }
        if let Some(r) = &cfg.restart {
            parse_restart(r).with_context(|| format!("{}: restart", path.display()))?;
        }
        if cfg.memory_alarm.is_some_and(|p| p > 100) {
            bail!("{}: memory_alarm is a percentage (0-100)", path.display());
        }
        for step in &cfg.serve {
            let c = match step {
                Step::Run(c) | Step::When { run: c, .. } => c,
            };
            if c != "start" && !cfg.commands.contains_key(c) {
                bail!("{}: serve step `{c}` is not `start` or a [commands] entry", path.display());
            }
        }
        Ok(cfg)
    }
}

pub const BUILTIN_COMMANDS: &[&str] = &["start", "serve", "eval", "vm", "info", "help"];

/// `restart`: true for "on-failure", false for "off" or "no".
pub fn parse_restart(s: &str) -> Result<bool> {
    match s.trim() {
        "on-failure" => Ok(true),
        "off" | "no" | "" => Ok(false),
        other => bail!("bad restart {other:?} (on-failure or off)"),
    }
}

/// The default `memory_alarm`, in percent of the VM's memory limit.
pub const MEMORY_ALARM: u8 = 80;

/// An `-eval` for `start`: a process that sets OTP's `{system_memory_high_watermark, []}` alarm,
/// as os_mon's memsup would, once the VM's memory reaches `pct` percent of its limit, and clears it
/// 10 points lower. The limit is MemTotal in the runtime's /proc/meminfo; what counts is
/// `erlang:memory(total)`, which falls again as memory is freed (the wasm memory itself never
/// shrinks). memsup stays off here: its port program would run on the host and measure the host's
/// memory, not the VM's limit.
pub fn memory_alarm_eval(pct: u8) -> String {
    format!(
        r#"Pct = {pct},
Limit = fun() ->
    case file:read_file("/proc/meminfo") of
        {{ok, <<"MemTotal:", Rest/binary>>}} -> [Kb | _] = string:lexemes(Rest, " \n"), binary_to_integer(Kb) * 1024;
        _ -> undefined
    end
end,
Watch = fun Loop(Up) ->
    Total = erlang:memory(total),
    Next = case Limit() of
        L when is_integer(L), not Up, Total * 100 >= L * Pct ->
            logger:warning("ampoule: VM memory at ~B% of its ~B MiB limit; alarm system_memory_high_watermark set", [Total * 100 div L, L bsr 20]),
            catch alarm_handler:set_alarm({{system_memory_high_watermark, []}}),
            true;
        L when is_integer(L), Up, Total * 100 < L * (Pct - 10) ->
            logger:notice("ampoule: VM memory back to ~B% of its limit; alarm system_memory_high_watermark cleared", [Total * 100 div L]),
            catch alarm_handler:clear_alarm(system_memory_high_watermark),
            false;
        _ -> Up
    end,
    receive after 500 -> Loop(Next) end
end,
register(ampoule_memory_alarm, spawn(fun() -> process_flag(priority, high), Watch(false) end))."#
    )
}

/// Embedded in every packed binary (JSON), next to the release's files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub name: String,
    pub vsn: String,
    pub erts_vsn: String,
    /// The release includes Elixir: commands go through `elixir start_cli` and `--eval`.
    pub elixir: bool,
    pub commands: BTreeMap<String, String>,
    pub serve: Vec<Step>,
    pub env: BTreeMap<String, String>,
    pub start_env: BTreeMap<String, String>,
    pub network: Network,
    /// `[process]` from ampoule.toml.
    #[serde(default)]
    pub process: Process,
    /// NIFs linked into the payload's engine (by library name).
    #[serde(default)]
    pub nifs: Vec<String>,
    /// `vm_args` from ampoule.toml.
    #[serde(default)]
    pub vm_args: String,
    /// `idle_stop` from ampoule.toml.
    #[serde(default)]
    pub idle_stop: Option<String>,
    /// `restart` from ampoule.toml.
    #[serde(default)]
    pub restart: Option<String>,
    /// `memory_alarm` from ampoule.toml.
    #[serde(default)]
    pub memory_alarm: Option<u8>,
}

impl Manifest {
    pub const FORMAT: u32 = 2;

    pub fn new(release: &ReleaseInfo, cfg: Config) -> Manifest {
        let serve = if cfg.serve.is_empty() { vec![Step::Run("start".into())] } else { cfg.serve };
        Manifest {
            format: Self::FORMAT,
            name: release.name.clone(),
            vsn: release.vsn.clone(),
            erts_vsn: release.erts_vsn.clone(),
            elixir: release.elixir,
            commands: cfg.commands,
            serve,
            env: cfg.env,
            start_env: cfg.start.env,
            network: cfg.network,
            process: cfg.process,
            nifs: Vec::new(),
            vm_args: cfg.vm_args,
            idle_stop: cfg.idle_stop,
            restart: cfg.restart,
            memory_alarm: cfg.memory_alarm,
        }
    }
}

/// Facts read from a release directory.
pub struct ReleaseInfo {
    pub name: String,
    pub vsn: String,
    pub erts_vsn: String,
    pub elixir: bool,
}

impl ReleaseInfo {
    pub fn from_dir(dir: &Path) -> Result<ReleaseInfo> {
        let start_erl = std::fs::read_to_string(dir.join("releases/start_erl.data"))
            .with_context(|| format!("{} is not a release (no releases/start_erl.data); run `mix release` first", dir.display()))?;
        let mut parts = start_erl.split_whitespace();
        let erts_vsn = parts.next().context("empty start_erl.data")?.to_string();
        let vsn = parts.next().context("start_erl.data has no release version")?.to_string();
        let vsn_dir = dir.join("releases").join(&vsn);
        let name = std::fs::read_dir(&vsn_dir)?
            .filter_map(|e| e.ok())
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".rel")).map(String::from))
            .next()
            .with_context(|| format!("no .rel file in {}", vsn_dir.display()))?;
        let elixir = std::fs::read_dir(dir.join("lib"))?.filter_map(|e| e.ok()).any(|e| e.file_name().to_string_lossy().starts_with("elixir-"));
        Ok(ReleaseInfo { name, vsn, erts_vsn, elixir })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(toml: &str) -> Result<Config> {
        let dir = crate::TestDir::new("config");
        let path = dir.0.join("ampoule.toml");
        std::fs::write(&path, toml)?;
        Config::load(&path)
    }

    #[test]
    fn step_when_var_runs_when_set_and_non_empty() {
        std::env::set_var("AMPOULE_TEST_STEP_SET", "1");
        let step = Step::When { run: "seed".into(), when: "AMPOULE_TEST_STEP_SET".into() };
        assert_eq!(step.command(), Some("seed"));
    }

    #[test]
    fn step_when_value_skips_on_mismatch() {
        std::env::set_var("AMPOULE_TEST_STEP_VALUE", "false");
        let step = Step::When { run: "seed".into(), when: "AMPOULE_TEST_STEP_VALUE=true".into() };
        assert_eq!(step.command(), None);
    }

    #[test]
    fn config_parses_serve_steps_and_commands() {
        let cfg =
            load("serve = [\"migrate\", { run = \"seed\", when = \"SEED\" }, \"start\"]\n[commands]\nmigrate = \"M.migrate\"\nseed = \"M.seed\"\n").unwrap();
        assert_eq!(cfg.serve.len(), 3);
    }

    #[test]
    fn config_rejects_redefining_a_builtin() {
        assert!(load("[commands]\nstart = \"x\"\n").is_err());
    }

    #[test]
    fn config_rejects_unknown_serve_steps() {
        assert!(load("serve = [\"migrate\", \"start\"]\n").is_err());
    }

    #[test]
    fn config_reads_process_allow() {
        let cfg = load("[process]\nallow = [\"git\", \"/bin/sh\", \"*\"]\n").unwrap();
        assert_eq!(cfg.process.allow, vec!["git", "/bin/sh", "*"]);
    }

    #[test]
    fn priv_programs_run_unless_switched_off() {
        let (on, off) = (load("[process]\nallow = [\"git\"]\n").unwrap(), load("[process]\npriv = false\n").unwrap());
        assert!(on.process.run_priv && Config::default().process.run_priv && !off.process.run_priv);
        // A manifest packed before the key existed keeps the default.
        let old: Process = serde_json::from_str(r#"{"allow": []}"#).unwrap();
        assert!(old.run_priv);
    }

    #[test]
    fn config_rejects_relative_paths_in_process_allow() {
        assert!(load("[process]\nallow = [\"bin/tool\"]\n").is_err());
    }

    #[test]
    fn restart_takes_on_failure_or_off() {
        assert!(parse_restart("on-failure").unwrap() && !parse_restart("off").unwrap() && parse_restart("always").is_err());
    }

    #[test]
    fn config_rejects_a_memory_alarm_above_100() {
        assert!(load("memory_alarm = 150\n").is_err() && load("memory_alarm = 75\nrestart = \"on-failure\"\n").is_ok());
    }

    #[test]
    fn config_rejects_unknown_keys() {
        assert!(load("[network]\nlisten = [4000]\nports = [1]\n").is_err());
    }
}
