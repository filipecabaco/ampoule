//! Erlang distribution: `[distribution]` in ampoule.toml, `--node`, `--distribution`,
//! `--dist-port`, `--epmd`, `--cookie-file` and their variables (`RELEASE_NODE`,
//! `RELEASE_DISTRIBUTION`, `RELEASE_COOKIE` as `mix release` reads them).
//!
//! The runtime starts beam without erlexec, so no epmd is started. Two ways to find peers:
//!
//! - `epmd = "none"` (the default): every node listens on one known port (9100) and reaches its
//!   peers on that same port: OTP's `-erl_epmd_port`. Registering with an epmd is still tried,
//!   and failing it is not an error.
//! - `epmd = "host"`: the node registers with an epmd already running on the host, on a port of
//!   its own (or `port`), as a native release does.
//!
//! Distribution applies to `start`. `remote` and `rpc` start a hidden node that does not listen
//! (`-dist_listen false`) and connects to the running one with the same cookie.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::Distribution;

/// The distribution port without epmd, unless configured.
pub const PORT: u16 = 9100;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Name,
    Sname,
    None,
}

impl Mode {
    pub fn parse(s: &str) -> Result<Mode> {
        match s.trim() {
            "name" => Ok(Mode::Name),
            "sname" => Ok(Mode::Sname),
            "none" | "" => Ok(Mode::None),
            other => bail!("bad distribution {other:?} (name, sname or none)"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Epmd {
    None,
    Host,
}

impl Epmd {
    pub fn parse(s: &str) -> Result<Epmd> {
        match s.trim() {
            "none" => Ok(Epmd::None),
            "host" => Ok(Epmd::Host),
            other => bail!("bad epmd {other:?} (none or host)"),
        }
    }
}

/// What the node starts as.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Role {
    /// `start`: the app's node, listening for peers.
    Node,
    /// `remote` ("rem"), `rpc` ("rpc"): a hidden node that only connects out to the app's node,
    /// named after the app's with this prefix and a random tag, as `mix release` names it.
    Client(&'static str),
}

/// The settings from the manifest, then variables, then flags; a later one wins.
#[derive(Debug, Clone, Default)]
pub struct Opts {
    pub mode: Option<Mode>,
    pub node: Option<String>,
    pub port: Option<u16>,
    pub epmd: Option<Epmd>,
    /// `--cookie-file` or `AMPOULE_COOKIE_FILE`.
    pub cookie_file: Option<PathBuf>,
    /// `cookie_file` in ampoule.toml: below `RELEASE_COOKIE`.
    pub config_cookie_file: Option<PathBuf>,
}

impl Opts {
    pub fn from_config(d: &Distribution) -> Result<Opts> {
        let o = Opts {
            mode: d.mode.as_deref().map(Mode::parse).transpose()?,
            node: d.node.clone(),
            port: d.port,
            epmd: d.epmd.as_deref().map(Epmd::parse).transpose()?,
            cookie_file: None,
            config_cookie_file: d.cookie_file.as_ref().map(PathBuf::from),
        };
        o.check()?;
        Ok(o)
    }

    /// Apply a runtime option; false if `key` is not a distribution option.
    pub fn apply(&mut self, key: &str, v: &str) -> Result<bool> {
        match key {
            "distribution" => self.mode = Some(Mode::parse(v)?),
            "node" => self.node = Some(v.trim().to_string()).filter(|n| !n.is_empty()),
            "dist-port" => self.port = Some(v.trim().parse().with_context(|| format!("bad port {v}"))?),
            "epmd" => self.epmd = Some(Epmd::parse(v)?),
            "cookie-file" => self.cookie_file = Some(PathBuf::from(v)),
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn check(&self) -> Result<()> {
        if let Some(node) = &self.node {
            check_node(node)?;
        }
        if let Some(d) = self.resolve("app") {
            if !d.long && d.node.split_once('@').is_some_and(|(_, h)| h.contains('.')) {
                bail!("node {}: a short name (sname) cannot have a dotted host; use mode = \"name\"", d.node);
            }
            if d.epmd == Epmd::None && d.port == Some(0) {
                bail!("the distribution port cannot be 0 without epmd: peers would not know it");
            }
        }
        Ok(())
    }

    /// The node to run, or None when distribution is off. Setting a node turns it on: `-name`
    /// when its host has a dot or a colon (an FQDN or an IP address), `-sname` otherwise.
    pub fn resolve(&self, release: &str) -> Option<Dist> {
        let mode = match self.mode {
            Some(m) => m,
            None => match &self.node {
                Some(n) if n.split_once('@').is_some_and(|(_, h)| h.contains(['.', ':'])) => Mode::Name,
                Some(_) => Mode::Sname,
                None => Mode::None,
            },
        };
        let long = match mode {
            Mode::Name => true,
            Mode::Sname => false,
            Mode::None => return None,
        };
        let epmd = self.epmd.unwrap_or(Epmd::None);
        Some(Dist {
            long,
            node: self.node.clone().unwrap_or_else(|| release.to_string()),
            port: self.port,
            epmd,
            cookie_file: self.cookie_file.clone(),
            config_cookie_file: self.config_cookie_file.clone(),
        })
    }
}

/// `name` or `name@host`, as erl takes it.
fn check_node(node: &str) -> Result<()> {
    let name = node.split_once('@').map_or(node, |(n, _)| n);
    if name.is_empty() || node.ends_with('@') || node.contains(char::is_whitespace) || node.matches('@').count() > 1 {
        bail!("bad node name {node:?} (NAME or NAME@HOST)");
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dist {
    /// `-name` (else `-sname`).
    pub long: bool,
    pub node: String,
    /// None: 9100 without epmd, any free port with the host's.
    pub port: Option<u16>,
    pub epmd: Epmd,
    pub cookie_file: Option<PathBuf>,
    pub config_cookie_file: Option<PathBuf>,
}

impl Dist {
    /// The port the node listens on, if it is a fixed one.
    pub fn listen_port(&self) -> Option<u16> {
        match self.epmd {
            Epmd::None => Some(self.port.unwrap_or(PORT)),
            Epmd::Host => self.port.filter(|p| *p != 0),
        }
    }

    /// The app's node with its host: what `remote` and `rpc` connect to. The guest's host name is
    /// the host's (`gethostname`), the part before the first dot for a short name.
    pub fn target(&self, hostname: &str) -> String {
        if self.node.contains('@') {
            return self.node.clone();
        }
        let host = if self.long { hostname } else { hostname.split('.').next().unwrap_or(hostname) };
        format!("{}@{host}", self.node)
    }

    /// Init arguments for `role`. `cookie` goes on the guest's command line, which the host's
    /// `ps` does not show; `epmd_port` is the host's ERL_EPMD_PORT, for the host's epmd.
    pub fn args(&self, role: Role, cookie: &str, epmd_port: Option<&str>) -> Vec<String> {
        let flag = if self.long { "-name" } else { "-sname" };
        let mut a: Vec<String> = Vec::new();
        match role {
            Role::Node => a.extend([flag.into(), self.node.clone()]),
            Role::Client(prefix) => {
                let tag = std::process::id() ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
                a.extend([flag.into(), format!("{prefix}-{:04x}-{}", tag & 0xffff, self.node), "-hidden".into(), "-dist_listen".into(), "false".into()]);
            },
        }
        a.extend(["-setcookie".into(), cookie.into()]);
        match self.epmd {
            // Where the node listens and where it reaches its peers.
            Epmd::None => a.extend(["-erl_epmd_port".into(), self.port.unwrap_or(PORT).to_string()]),
            Epmd::Host => {
                if let (Role::Node, Some(p)) = (role, self.listen_port()) {
                    a.extend(["-kernel", "inet_dist_listen_min", &p.to_string(), "inet_dist_listen_max", &p.to_string()].map(String::from));
                }
                if let Some(p) = epmd_port {
                    a.extend(["-epmd_port".into(), p.into()]);
                }
            },
        }
        a
    }

    /// The cookie: `--cookie-file`, then `RELEASE_COOKIE`, then `cookie_file` in ampoule.toml,
    /// then the release's own `releases/COOKIE` (as `mix release` does). Returns it and where it
    /// came from (never the cookie itself in a message).
    pub fn cookie(&self, env: Option<String>, release_cookie: Option<String>) -> Result<(String, String)> {
        let (cookie, source) = if let Some(f) = &self.cookie_file {
            (read_cookie(f)?, f.display().to_string())
        } else if let Some(c) = env.filter(|c| !c.is_empty()) {
            (c, "RELEASE_COOKIE".into())
        } else if let Some(f) = &self.config_cookie_file {
            (read_cookie(f)?, f.display().to_string())
        } else if let Some(c) = release_cookie {
            (c, "the release's releases/COOKIE".into())
        } else {
            bail!("distribution needs a cookie: set RELEASE_COOKIE, pass --cookie-file FILE, or set cookie_file in [distribution]");
        };
        let cookie = cookie.trim().to_string();
        if cookie.is_empty() {
            bail!("the cookie from {source} is empty");
        }
        // init would read it as the next flag, leaving -setcookie without a value.
        if cookie.starts_with('-') {
            bail!("the cookie from {source} starts with '-', which erl cannot take as a cookie");
        }
        Ok((cookie, source))
    }
}

fn read_cookie(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading the cookie file {}", path.display()))
}

/// Distribution flags from vm.args: what passes through when the node is distributed, and the
/// port of an `-erl_epmd_port` (mix release's way to run without epmd).
#[derive(Debug, Default, PartialEq)]
pub struct FromVmArgs {
    pub keep: Vec<String>,
    pub ignored: Vec<String>,
    pub port: Option<u16>,
}

/// Sort the distribution flags vmargs collected. The node, the cookie and how epmd is used come
/// from ampoule's own options; `-hidden`, `-proto_dist`, `-epmd_module` and `-dist_listen` still
/// apply.
pub fn from_vm_args(flags: &[(String, Option<String>)]) -> FromVmArgs {
    let mut out = FromVmArgs::default();
    for (flag, value) in flags {
        match flag.as_str() {
            "-hidden" | "-proto_dist" | "-epmd_module" | "-dist_listen" => {
                out.keep.push(flag.clone());
                out.keep.extend(value.clone());
            },
            "-erl_epmd_port" => out.port = value.as_deref().and_then(|v| v.parse().ok()),
            // Meaningless without erlexec.
            "-start_epmd" | "-epmd" | "-no_epmd" => {},
            _ => out.ignored.push(shown(flag, value.as_deref())),
        }
    }
    out
}

/// A flag as a warning shows it: never a cookie.
pub fn shown(flag: &str, value: Option<&str>) -> String {
    match (flag, value) {
        ("-setcookie", Some(_)) => "-setcookie (hidden)".into(),
        (f, Some(v)) => format!("{f} {v}"),
        (f, None) => f.into(),
    }
}

/// /etc/hosts entries for the host's own name, so a node named after it (`-sname`, or `-name`
/// without a host) resolves inside the guest, whose resolver reads only /etc/hosts and DNS. The
/// name maps to 127.0.0.1, as Debian's /etc/hosts maps it, unless the host's file already has it.
pub fn self_hosts(hostname: &str, hosts_text: &str) -> Vec<(String, String)> {
    let known: std::collections::HashSet<&str> =
        hosts_text.lines().map(|l| l.split('#').next().unwrap_or("")).flat_map(|l| l.split_whitespace().skip(1)).collect();
    let short = hostname.split('.').next().unwrap_or(hostname);
    let mut out = Vec::new();
    for name in [hostname, short] {
        if !name.is_empty() && !known.contains(name) && !out.iter().any(|(n, _): &(String, String)| n == name) {
            out.push((name.to_string(), "127.0.0.1".to_string()));
        }
    }
    out
}

/// The host's name, as the guest's `gethostname` reports it.
pub fn hostname() -> String {
    rustix::system::uname().nodename().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(node: Option<&str>, mode: Option<Mode>) -> Opts {
        Opts { node: node.map(String::from), mode, ..Opts::default() }
    }

    #[test]
    fn off_unless_a_mode_or_a_node_is_set() {
        assert_eq!(Opts::default().resolve("app"), None);
        assert_eq!(opts(Some("a@10.0.0.1"), Some(Mode::None)).resolve("app"), None);
        let d = opts(None, Some(Mode::Sname)).resolve("app").unwrap();
        assert_eq!((d.long, d.node.as_str(), d.listen_port()), (false, "app", Some(PORT)));
    }

    #[test]
    fn a_node_alone_picks_name_or_sname_from_its_host() {
        assert!(opts(Some("a@10.0.0.1"), None).resolve("app").unwrap().long);
        assert!(opts(Some("a@host.example.com"), None).resolve("app").unwrap().long);
        assert!(opts(Some("a@::1"), None).resolve("app").unwrap().long);
        assert!(!opts(Some("a@box"), None).resolve("app").unwrap().long);
        assert!(!opts(Some("a"), None).resolve("app").unwrap().long);
    }

    #[test]
    fn options_parse_and_reject_garbage() {
        let mut o = Opts::default();
        for (k, v) in [("distribution", "name"), ("node", "a@10.0.0.1"), ("dist-port", "9200"), ("epmd", "host"), ("cookie-file", "/c")] {
            assert!(o.apply(k, v).unwrap());
        }
        assert_eq!((o.mode, o.port, o.epmd, o.cookie_file.clone()), (Some(Mode::Name), Some(9200), Some(Epmd::Host), Some(PathBuf::from("/c"))));
        assert!(!o.apply("listen", "1").unwrap());
        assert!(o.apply("distribution", "long").is_err() && o.apply("epmd", "yes").is_err() && o.apply("dist-port", "x").is_err());
    }

    #[test]
    fn check_refuses_names_erl_would_refuse() {
        assert!(opts(Some("a@b@c"), None).check().is_err());
        assert!(opts(Some("@host"), None).check().is_err());
        assert!(opts(Some("a b"), None).check().is_err());
        assert!(opts(Some("a@10.0.0.1"), Some(Mode::Sname)).check().is_err());
        assert!(Opts { port: Some(0), ..opts(Some("a"), None) }.check().is_err());
        assert!(Opts { port: Some(0), epmd: Some(Epmd::Host), ..opts(Some("a"), None) }.check().is_ok());
    }

    #[test]
    fn node_args_without_epmd_fix_the_port() {
        let d = opts(Some("a@127.0.0.1"), None).resolve("app").unwrap();
        assert_eq!(d.args(Role::Node, "c", Some("4370")), ["-name", "a@127.0.0.1", "-setcookie", "c", "-erl_epmd_port", "9100"]);
    }

    #[test]
    fn node_args_with_the_hosts_epmd() {
        let mut d = opts(Some("a"), None).resolve("app").unwrap();
        d.epmd = Epmd::Host;
        assert_eq!(d.args(Role::Node, "c", None), ["-sname", "a", "-setcookie", "c"]);
        d.port = Some(9200);
        assert_eq!(
            d.args(Role::Node, "c", Some("4370")),
            ["-sname", "a", "-setcookie", "c", "-kernel", "inet_dist_listen_min", "9200", "inet_dist_listen_max", "9200", "-epmd_port", "4370"]
        );
    }

    #[test]
    fn client_args_are_hidden_and_do_not_listen() {
        let d = Opts { port: Some(9300), ..opts(Some("a@127.0.0.1"), None) }.resolve("app").unwrap();
        let a = d.args(Role::Client("rem"), "c", None);
        assert!(a[1].starts_with("rem-") && a[1].ends_with("-a@127.0.0.1"));
        assert_eq!(a[2..], ["-hidden", "-dist_listen", "false", "-setcookie", "c", "-erl_epmd_port", "9300"]);
    }

    #[test]
    fn target_adds_the_host() {
        let short = opts(Some("a"), None).resolve("app").unwrap();
        let long = opts(Some("a"), Some(Mode::Name)).resolve("app").unwrap();
        assert_eq!((short.target("box.lan"), long.target("box.lan")), ("a@box".to_string(), "a@box.lan".to_string()));
        assert_eq!(opts(Some("a@10.0.0.1"), None).resolve("app").unwrap().target("box"), "a@10.0.0.1");
    }

    #[test]
    fn cookie_sources_in_order() {
        let dir = crate::TestDir::new("cookie");
        let (flag, cfg) = (dir.0.join("flag"), dir.0.join("cfg"));
        std::fs::write(&flag, "from-flag\n").unwrap();
        std::fs::write(&cfg, "from-config").unwrap();
        let mut d = opts(Some("a"), None).resolve("app").unwrap();
        assert!(d.cookie(None, None).is_err());
        assert_eq!(d.cookie(None, Some("rel".into())).unwrap().0, "rel");
        d.config_cookie_file = Some(cfg);
        assert_eq!(d.cookie(None, Some("rel".into())).unwrap().0, "from-config");
        assert_eq!(d.cookie(Some("env".into()), Some("rel".into())).unwrap(), ("env".to_string(), "RELEASE_COOKIE".to_string()));
        d.cookie_file = Some(flag);
        assert_eq!(d.cookie(Some("env".into()), None).unwrap().0, "from-flag");
        d.cookie_file = Some(dir.0.join("missing"));
        assert!(d.cookie(Some("env".into()), None).is_err());
        let empty = dir.0.join("empty");
        std::fs::write(&empty, " \n").unwrap();
        d.cookie_file = Some(empty);
        let err = d.cookie(None, None).unwrap_err().to_string();
        assert!(err.contains("empty"));
        d.cookie_file = None;
        let err = d.cookie(Some("-n secret".into()), None).unwrap_err().to_string();
        assert!(err.contains("starts with '-'") && !err.contains("secret"));
    }

    #[test]
    fn vm_args_keep_what_still_applies_and_never_show_the_cookie() {
        let f = |f: &str, v: Option<&str>| (f.to_string(), v.map(String::from));
        let out = from_vm_args(&[
            f("-sname", Some("x")),
            f("-setcookie", Some("secret")),
            f("-start_epmd", Some("false")),
            f("-erl_epmd_port", Some("6789")),
            f("-hidden", None),
            f("-proto_dist", Some("inet6_tcp")),
        ]);
        assert_eq!(out.keep, ["-hidden", "-proto_dist", "inet6_tcp"]);
        assert_eq!(out.ignored, ["-sname x", "-setcookie (hidden)"]);
        assert_eq!(out.port, Some(6789));
    }

    #[test]
    fn self_hosts_adds_the_host_name_once() {
        assert_eq!(
            self_hosts("box.lan", "127.0.0.1 localhost\n"),
            [("box.lan".to_string(), "127.0.0.1".to_string()), ("box".to_string(), "127.0.0.1".to_string())]
        );
        assert_eq!(self_hosts("box", "10.0.0.5\tbox # me\n"), Vec::<(String, String)>::new());
        assert_eq!(self_hosts("box", "# 10.0.0.5 box\n").len(), 1);
    }
}
