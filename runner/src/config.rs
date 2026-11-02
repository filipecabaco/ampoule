//! What a packed binary knows about its app: the manifest, built at pack time from the release
//! itself and an optional config file (`ampoule.toml`):
//!
//! ```toml
//! serve = ["migrate", { run = "seed", when = "SEED=true" }, "start"]   # `<binary> serve`
//! vm_args = "+S :4 +SDio 2"        # VM flags as in vm.args, after the release's own
//! idle_stop = "5m"               # `start` stops the VM after this long without connections
//! restart = "on-failure"         # `start` starts the VM again when it fails (crash, out of memory)
//! memory_alarm = 80              # set system_memory_high_watermark at this % of the VM's memory limit
//! native_cache = "off"           # keep the compiled engine between starts: "auto" (default), "off", or a directory
//! home = "host"                  # the app's home is the host's HOME, not its private /tmp
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
//!
//! [distribution]                   # Erlang distribution for `start` (default: off)
//! mode = "name"                    # "name", "sname" or "none"
//! node = "my_app@10.0.0.5"         # default: the release name
//! port = 9100                      # the distribution port (default: 9100 without epmd)
//! epmd = "none"                    # "none": every node on `port`; "host": the host's epmd
//! cookie_file = "/run/secrets/erlang-cookie"
//!
//! [[mounts]]                       # host paths the app sees; --mount and AMPOULE_MOUNTS replace them
//! host = "$PWD"                    # `~`, `$VAR` and `${VAR}` expand where the binary runs
//!
//! [[mounts]]
//! host = "~/.config/my_app"
//! guest = "/config"                # default: the expanded host path
//! ro = false
//! create = true                    # create the host directory (0700) when it is missing
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

/// `[[mounts]]`: a host path the app sees. Both paths expand (`expand`) where the binary runs, not
/// where it is packed, so a mount of `~/.config/my_app` or `$PWD` follows the user.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mount {
    pub host: String,
    /// The guest path; unset: the expanded host path, so the app sees the host's path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest: Option<String>,
    #[serde(default)]
    pub ro: bool,
    /// Create the host directory, and its missing parents, with mode 0700 when it is missing.
    #[serde(default)]
    pub create: bool,
}

impl Mount {
    /// Checks both paths expand, with every variable set.
    pub fn check(&self) -> Result<()> {
        let any = |_: &str| Some("x".to_string());
        if self.host.trim().is_empty() {
            bail!("a mount needs a host path");
        }
        expand(&self.host, "/home", any)?;
        if let Some(g) = &self.guest {
            expand(g, "/home", any)?;
        }
        Ok(())
    }
}

/// Expands a leading `~` (alone or before `/`) to `home`, and `$VAR` and `${VAR}` to `var(VAR)`.
/// An unset variable is an error, not an empty string: `$UNSET/data` must not become `/data`.
/// A `$` that starts no variable name stays as it is.
pub fn expand(s: &str, home: &str, var: impl Fn(&str) -> Option<String>) -> Result<String> {
    let rest = match s.strip_prefix('~') {
        Some(r) if r.is_empty() || r.starts_with('/') => {
            if home.is_empty() {
                bail!("`{s}`: ~ needs HOME");
            }
            return Ok(format!("{home}{}", expand(r, home, var)?));
        },
        _ => s,
    };
    let name_char = |c: char| c == '_' || c.is_ascii_alphanumeric();
    let mut out = String::with_capacity(rest.len());
    let mut chars = rest.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        let tail = &rest[i + 1..];
        let (name, used) = if let Some(braced) = tail.strip_prefix('{') {
            let end = braced.find('}').with_context(|| format!("`{s}`: unclosed ${{"))?;
            (&braced[..end], end + 2)
        } else {
            let end = tail.find(|c: char| !name_char(c)).unwrap_or(tail.len());
            (&tail[..end], end)
        };
        if name.is_empty() || !name.chars().all(name_char) || name.starts_with(|c: char| c.is_ascii_digit()) {
            if tail.starts_with('{') {
                bail!("`{s}`: bad variable name {name:?}");
            }
            out.push('$');
            continue;
        }
        out.push_str(&var(name).with_context(|| format!("`{s}`: {name} is not set"))?);
        for _ in 0..used {
            chars.next();
        }
    }
    Ok(out)
}

/// `home`: true for "host" (the app's home is the host's HOME), false for "private" (its /tmp).
pub fn parse_home(s: &str) -> Result<bool> {
    match s.trim() {
        "host" => Ok(true),
        "private" | "" => Ok(false),
        other => bail!("bad home {other:?} (host or private)"),
    }
}

/// `[distribution]`: see `dist`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Distribution {
    /// "name", "sname" or "none" (RELEASE_DISTRIBUTION); unset: on when `node` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// "none" or "host".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epmd: Option<String>,
    /// A host file holding the cookie, read when the binary runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie_file: Option<String>,
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
    #[serde(default)]
    pub distribution: Distribution,
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
    /// Where the compiled engine is kept between starts: "auto", "off" or a directory (`native_cache`).
    #[serde(default)]
    pub native_cache: Option<String>,
    /// "host": the app's home is the host's HOME; "private" (the default): its /tmp.
    #[serde(default)]
    pub home: Option<String>,
    #[serde(default)]
    pub mounts: Vec<Mount>,
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
        crate::dist::Opts::from_config(&cfg.distribution).with_context(|| format!("{}: [distribution]", path.display()))?;
        if let Some(d) = &cfg.idle_stop {
            crate::idle::parse_duration(d).with_context(|| format!("{}: idle_stop", path.display()))?;
        }
        if let Some(r) = &cfg.restart {
            parse_restart(r).with_context(|| format!("{}: restart", path.display()))?;
        }
        if let Some(c) = &cfg.native_cache {
            crate::native_cache::Setting::parse(c).with_context(|| format!("{}: native_cache", path.display()))?;
        }
        if let Some(h) = &cfg.home {
            parse_home(h).with_context(|| format!("{}: home", path.display()))?;
        }
        for m in &cfg.mounts {
            m.check().with_context(|| format!("{}: [[mounts]]", path.display()))?;
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

pub const BUILTIN_COMMANDS: &[&str] = &["start", "serve", "eval", "remote", "rpc", "vm", "info", "help"];

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
///
/// The limit is read once: it does not change while the VM runs. Without one (no MemTotal) there
/// is nothing to measure against, and the process only waits. The watch looks twice a second once
/// memory is past half the alarm level, and every 5 seconds below that, so an idle VM spends next
/// to nothing on it: while memory is below half the alarm level, the alarm can come up to 5
/// seconds after it is reached.
pub fn memory_alarm_eval(pct: u8) -> String {
    format!(
        r#"Pct = {pct},
Limit = fun() ->
    case file:read_file("/proc/meminfo") of
        {{ok, <<"MemTotal:", Rest/binary>>}} -> [Kb | _] = string:lexemes(Rest, " \n"), binary_to_integer(Kb) * 1024;
        _ -> undefined
    end
end,
Watch = fun Loop(Up, L) ->
    Total = erlang:memory(total),
    Next = if
        not Up, Total * 100 >= L * Pct ->
            logger:warning("ampoule: VM memory at ~B% of its ~B MiB limit; alarm system_memory_high_watermark set", [Total * 100 div L, L bsr 20]),
            catch alarm_handler:set_alarm({{system_memory_high_watermark, []}}),
            true;
        Up, Total * 100 < L * (Pct - 10) ->
            logger:notice("ampoule: VM memory back to ~B% of its limit; alarm system_memory_high_watermark cleared", [Total * 100 div L]),
            catch alarm_handler:clear_alarm(system_memory_high_watermark),
            false;
        true -> Up
    end,
    Every = if
        not Next, Total * 200 < L * Pct -> 5000;
        true -> 500
    end,
    receive after Every -> Loop(Next, L) end
end,
register(ampoule_memory_alarm, spawn(fun() ->
    process_flag(priority, high),
    case Limit() of
        L when is_integer(L), L > 0 -> Watch(false, L);
        _ -> receive after infinity -> ok end
    end
end))."#
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
    /// `[distribution]` from ampoule.toml.
    #[serde(default)]
    pub distribution: Distribution,
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
    /// `native_cache` from ampoule.toml.
    #[serde(default)]
    pub native_cache: Option<String>,
    /// `home` from ampoule.toml.
    #[serde(default)]
    pub home: Option<String>,
    /// `[[mounts]]` from ampoule.toml, not yet expanded.
    #[serde(default)]
    pub mounts: Vec<Mount>,
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
            distribution: cfg.distribution,
            nifs: Vec::new(),
            vm_args: cfg.vm_args,
            idle_stop: cfg.idle_stop,
            restart: cfg.restart,
            memory_alarm: cfg.memory_alarm,
            native_cache: cfg.native_cache,
            home: cfg.home,
            mounts: cfg.mounts,
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
    fn config_takes_a_native_cache_setting() {
        assert_eq!(load("native_cache = \"off\"\n").unwrap().native_cache.as_deref(), Some("off"));
        assert!(load("native_cache = \"/srv/engines\"\n").is_ok());
        assert!(load("native_cache = \"relative\"\n").is_err());
    }

    #[test]
    fn config_rejects_a_memory_alarm_above_100() {
        assert!(load("memory_alarm = 150\n").is_err() && load("memory_alarm = 75\nrestart = \"on-failure\"\n").is_ok());
    }

    #[test]
    fn config_reads_distribution() {
        let cfg = load("[distribution]\nnode = \"app@10.0.0.5\"\nport = 9200\nepmd = \"host\"\ncookie_file = \"/run/secrets/cookie\"\n").unwrap();
        let d = cfg.distribution;
        assert_eq!(
            (d.node.as_deref(), d.port, d.epmd.as_deref(), d.cookie_file.as_deref()),
            (Some("app@10.0.0.5"), Some(9200), Some("host"), Some("/run/secrets/cookie"))
        );
    }

    #[test]
    fn config_rejects_bad_distribution_values() {
        assert!(load("[distribution]\nmode = \"long\"\n").is_err());
        assert!(load("[distribution]\nepmd = \"local\"\n").is_err());
        assert!(load("[distribution]\ncookie = \"secret\"\n").is_err());
        assert!(load("[distribution]\nmode = \"sname\"\nnode = \"app@10.0.0.5\"\n").is_err());
    }

    #[test]
    fn config_rejects_unknown_keys() {
        assert!(load("[network]\nlisten = [4000]\nports = [1]\n").is_err());
    }

    fn vars(name: &str) -> Option<String> {
        match name {
            "PWD" => Some("/work/repo".into()),
            "XDG_CONFIG_HOME" => Some("/home/me/.config".into()),
            _ => None,
        }
    }

    #[test]
    fn expand_replaces_a_leading_tilde_with_home() {
        assert_eq!(expand("~", "/home/me", vars).unwrap(), "/home/me");
        assert_eq!(expand("~/.config/app", "/home/me", vars).unwrap(), "/home/me/.config/app");
        assert_eq!(expand("~other/x", "/home/me", vars).unwrap(), "~other/x");
        assert_eq!(expand("/data/~", "/home/me", vars).unwrap(), "/data/~");
        assert!(expand("~/x", "", vars).is_err());
    }

    #[test]
    fn expand_replaces_variables_with_and_without_braces() {
        assert_eq!(expand("$PWD", "/h", vars).unwrap(), "/work/repo");
        assert_eq!(expand("${XDG_CONFIG_HOME}/app", "/h", vars).unwrap(), "/home/me/.config/app");
        assert_eq!(expand("$PWD/sub:$PWD", "/h", vars).unwrap(), "/work/repo/sub:/work/repo");
    }

    #[test]
    fn expand_refuses_an_unset_variable() {
        let e = expand("$UNSET/data", "/h", vars).unwrap_err().to_string();
        assert!(e.contains("UNSET is not set"), "{e}");
        assert!(expand("${UNSET}", "/h", vars).is_err());
    }

    #[test]
    fn expand_keeps_a_dollar_that_starts_no_variable() {
        assert_eq!(expand("/a$/b$", "/h", vars).unwrap(), "/a$/b$");
        assert_eq!(expand("/price$5", "/h", vars).unwrap(), "/price$5");
        assert!(expand("${PWD", "/h", vars).is_err());
        assert!(expand("${1X}", "/h", vars).is_err());
    }

    #[test]
    fn config_reads_mounts() {
        let cfg = load("[[mounts]]\nhost = \"$PWD\"\n\n[[mounts]]\nhost = \"~/.config/app\"\nguest = \"/config\"\nro = true\ncreate = true\n").unwrap();
        assert_eq!(
            cfg.mounts,
            vec![
                Mount { host: "$PWD".into(), guest: None, ro: false, create: false },
                Mount { host: "~/.config/app".into(), guest: Some("/config".into()), ro: true, create: true },
            ]
        );
    }

    #[test]
    fn config_rejects_bad_mounts() {
        assert!(load("[[mounts]]\nhost = \"\"\n").is_err());
        assert!(load("[[mounts]]\nhost = \"${PWD\"\n").is_err());
        assert!(load("[[mounts]]\nguest = \"/x\"\n").is_err());
        assert!(load("[[mounts]]\nhost = \"/x\"\nmode = \"rw\"\n").is_err());
    }

    #[test]
    fn home_takes_host_or_private() {
        assert!(parse_home("host").unwrap());
        assert!(!parse_home("private").unwrap());
        assert!(load("home = \"host\"\n").is_ok());
        assert!(load("home = \"shared\"\n").is_err());
    }

    #[test]
    fn the_manifest_keeps_home_and_the_mounts_unexpanded() {
        let cfg = load("home = \"host\"\n\n[[mounts]]\nhost = \"~/.config/app\"\ncreate = true\n").unwrap();
        let release = ReleaseInfo { name: "app".into(), vsn: "0.1.0".into(), erts_vsn: "16.0".into(), elixir: true };
        let json = serde_json::to_string(&Manifest::new(&release, cfg)).unwrap();
        let m: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m.home.as_deref(), Some("host"));
        assert_eq!(m.mounts[0].host, "~/.config/app");
        assert!(m.mounts[0].create);
    }
}
