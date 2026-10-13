//! What a packed binary knows about its app: the manifest, built at pack time from the release
//! itself and an optional config file (`ampoule.toml`):
//!
//! ```toml
//! serve = ["migrate", { run = "seed", when = "SEED=true" }, "start"]   # `<binary> serve`
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
    /// NIFs linked into the payload's engine (by library name).
    #[serde(default)]
    pub nifs: Vec<String>,
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
            nifs: Vec::new(),
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
    fn config_rejects_unknown_keys() {
        assert!(load("[network]\nlisten = [4000]\nports = [1]\n").is_err());
    }
}
