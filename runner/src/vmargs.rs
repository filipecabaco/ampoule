//! `-args_file`, applied the way erlexec applies it. The runtime starts beam directly, without
//! erlexec, so this is what reads a release's vm.args: `+` flags become emulator flags, `-env`
//! sets an environment variable, `-extra` starts plain arguments and everything else stays an
//! init argument where `-args_file` was. Distribution flags are set apart for `dist` to apply or
//! leave out, and `-detached` and `-heart` are dropped. Splitting and the table of flags that take
//! a value follow erts/etc/common/erlexec.c.

use anyhow::{bail, Result};

#[derive(Debug, Default, PartialEq)]
pub struct Expanded {
    /// Init arguments, `-args_file` replaced by what the files hold.
    pub erl: Vec<String>,
    /// Emulator flags, `+X` written as beam takes them (`-X`).
    pub emu: Vec<String>,
    /// Plain arguments from a file's `-extra`, for the end of the command line.
    pub extra: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Distribution flags (`-name`, `-setcookie`, `-erl_epmd_port`, ...) with their values.
    pub dist: Vec<(String, Option<String>)>,
    /// Flags left out, for a warning.
    pub dropped: Vec<String>,
}

/// Deeper than any real vm.args nests; stops a file that includes itself.
const MAX_DEPTH: usize = 16;

/// Expand every `-args_file FILE` in `args`. `read` returns a file's contents, or None if it does
/// not exist (skipped, so a release without vm.args still boots).
pub fn expand(args: &[String], read: &mut dyn FnMut(&str) -> Result<Option<String>>) -> Result<Expanded> {
    let mut out = Expanded::default();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-args_file" {
            let Some(file) = args.get(i + 1) else { bail!("-args_file needs a file name") };
            include(file, read, &mut out, 0)?;
            i += 2;
        } else {
            out.erl.push(args[i].clone());
            i += 1;
        }
    }
    Ok(out)
}

/// Add flags written as in an args file (`--vm-args`, `vm_args` in ampoule.toml) after what is
/// already in `out`; `source` names them in errors.
pub fn add(source: &str, text: &str, read: &mut dyn FnMut(&str) -> Result<Option<String>>, out: &mut Expanded) -> Result<()> {
    apply(source, text, read, out, 0)
}

fn include(file: &str, read: &mut dyn FnMut(&str) -> Result<Option<String>>, out: &mut Expanded, depth: usize) -> Result<()> {
    if depth >= MAX_DEPTH {
        bail!("{file}: -args_file nested more than {MAX_DEPTH} deep");
    }
    let Some(text) = read(file)? else { return Ok(()) };
    apply(file, &text, read, out, depth)
}

fn apply(file: &str, text: &str, read: &mut dyn FnMut(&str) -> Result<Option<String>>, out: &mut Expanded, depth: usize) -> Result<()> {
    let words = split(text);
    let mut i = 0;
    while i < words.len() {
        let w = &words[i];
        let next = words.get(i + 1);
        let take = |n: usize, what: &str| -> Result<Vec<String>> {
            match words.get(i + 1..i + 1 + n) {
                Some(v) => Ok(v.to_vec()),
                None => bail!("{file}: {w} needs {what}"),
            }
        };
        match w.as_str() {
            "-args_file" => {
                include(&take(1, "a file name")?[0], read, out, depth + 1)?;
                i += 2;
            },
            "-extra" => {
                out.extra.extend(words[i + 1..].iter().cloned());
                break;
            },
            "-env" => {
                let v = take(2, "a name and a value")?;
                out.env.push((v[0].clone(), v[1].clone()));
                i += 3;
            },
            "-name" | "-sname" | "-setcookie" | "-proto_dist" | "-start_epmd" | "-epmd_module" | "-epmd" | "-erl_epmd_port" | "-dist_listen" => {
                let v = take(1, "a value")?;
                out.dist.push((w.clone(), Some(v[0].clone())));
                i += 2;
            },
            "-hidden" | "-no_epmd" => {
                out.dist.push((w.clone(), None));
                i += 1;
            },
            "-detached" | "-heart" => {
                out.dropped.push(w.clone());
                i += 1;
            },
            _ if w.len() > 1 && w.starts_with('+') && !w.starts_with("++") => {
                let flag = &w[1..];
                out.emu.push(format!("-{flag}"));
                let value = match next {
                    Some(n) if takes_value(flag) => Some(n),
                    // +B and +c take an optional value.
                    Some(n) if flag == "B" && matches!(n.as_str(), "i" | "c" | "d") => Some(n),
                    Some(n) if flag == "c" && matches!(n.as_str(), "true" | "false") => Some(n),
                    None if takes_value(flag) => bail!("{file}: {w} needs a value"),
                    _ => None,
                };
                if let Some(v) = value {
                    out.emu.push(v.clone());
                    i += 1;
                }
                i += 1;
            },
            _ => {
                out.erl.push(w.clone());
                i += 1;
            },
        }
    }
    Ok(())
}

/// Rewrite relative scheduler counts in emulator flags (`-S`, `-SDcpu`) as absolute ones for a
/// host with `cores` cores. The emulator resolves a missing or non-positive count against the
/// processors it detects, and inside wasm it detects one, so `+S :6` or `+S -2` would fail or mean
/// something else; erl_init.c's rules applied to `cores` give what they mean on the host.
pub fn resolve_schedulers(emu: &mut [String], cores: usize) {
    let cores = cores as i64;
    let rel = |n: i64| if n > 0 { n } else { cores + n };
    // Only counts that depend on the processor count are rewritten. Online is capped at the total
    // there, so `+S :6` on a four-core host means 4:4 instead of a VM that refuses to start.
    let resolve = |v: &str| -> Option<String> {
        match v.split_once(':') {
            Some((tot, onln)) => {
                let t: i64 = if tot.is_empty() { 0 } else { tot.parse().ok()? };
                let o: i64 = onln.parse().ok()?;
                if t > 0 && o > 0 {
                    return None;
                }
                let t = rel(t);
                Some(format!("{t}:{}", rel(o).min(t)))
            },
            None => {
                let t: i64 = v.parse().ok()?;
                (t <= 0).then(|| rel(t).to_string())
            },
        }
    };
    let mut i = 0;
    while i < emu.len() {
        if emu[i] == "-S" || emu[i] == "-SDcpu" {
            if let Some(v) = emu.get(i + 1).and_then(|v| resolve(v)) {
                emu[i + 1] = v;
            }
            i += 2;
            continue;
        }
        // The value can also be attached: -S4:2, -SDcpu:2.
        let attached = emu[i]
            .strip_prefix("-SDcpu")
            .map(|v| ("-SDcpu", v))
            .or_else(|| emu[i].strip_prefix("-S").filter(|v| v.starts_with(|c: char| c.is_ascii_digit() || c == ':' || c == '-')).map(|v| ("-S", v)));
        if let Some((flag, v)) = attached.filter(|(_, v)| !v.is_empty()) {
            if let Some(v) = resolve(v) {
                emu[i] = format!("{flag}{v}");
            }
        }
        i += 1;
    }
}

/// Whether emulator flag `+flag` takes the next word as its value (erlexec's tables).
fn takes_value(flag: &str) -> bool {
    const AU_ALLOCS: &str = "uBIDEFHLRSTZ";
    const AU_SWITCHES: &[&str] = &[
        "as", "asbcst", "atags", "acul", "acnl", "acfml", "acful", "cp", "e", "t", "lmbcs", "mbcgs", "mbsd", "mmbcs", "mmmbc", "mmsbc", "msbclt", "ramv",
        "rmbcmt", "rsbcmt", "rsbcst", "sbct", "smbcs",
    ];
    const M_OTHER: &[&str] = &[
        "ea", "umadtn", "ummc", "uycs", "usac", "im", "is", "lpm", "Mamcbf", "Mrmcbf", "Mmcs", "Mscs", "Mscrfsd", "Msco", "Mscrpm", "Mlp", "Ye", "Ym", "Ytp",
        "Ytt", "Iscs", "Xscs", "dai",
    ];
    const S_LOWER: &[&str] =
        &["bt", "bwtdcpu", "bwtdio", "bwt", "cl", "ct", "ecio", "fwi", "tbt", "wct", "wtdcpu", "wtdio", "wt", "ws", "ss", "ssdcpu", "ssdio", "pp", "ub"];
    const H: &[&str] = &["ms", "mbs", "pds", "max", "maxk", "maxel", "maxib", "mqd", ""];
    const Z: &[&str] = &["dbbl", "dntgc", "ebwt", "osrl", "hft"];

    let mut chars = flag.chars();
    let Some(c) = chars.next() else { return false };
    let rest = chars.as_str();
    match c {
        'a' | 'A' | 'C' | 'e' | 'i' | 'n' | 'P' | 'Q' | 't' | 'T' | 'R' | 'W' | 'K' => rest.is_empty(),
        'D' => rest.starts_with('i'),
        'I' => matches!(rest, "Ot" | "Op" | "OPt" | "OPp" | "Os"),
        'J' => true,
        'S' => matches!(rest, "" | "P" | "Dcpu" | "DPcpu" | "Dio"),
        'M' => {
            let mut r = rest.chars();
            let alloc = r.next().is_some_and(|a| AU_ALLOCS.contains(a));
            (alloc && AU_SWITCHES.contains(&r.as_str())) || M_OTHER.contains(&rest)
        },
        'h' => H.contains(&rest),
        'r' => rest == "g",
        's' => S_LOWER.contains(&rest),
        'p' => matches!(rest, "c" | "ad"),
        'z' => Z.contains(&rest),
        _ => false,
    }
}

/// Split an args file into words: whitespace separates, `#` starts a comment to the end of the
/// line, quotes group, backslash escapes the next character.
pub fn split(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur: Option<String> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c' => words.extend(cur.take()),
            '#' => {
                words.extend(cur.take());
                for c in chars.by_ref() {
                    if c == '\n' {
                        break;
                    }
                }
            },
            '"' | '\'' => {
                let w = cur.get_or_insert_with(String::new);
                for q in chars.by_ref() {
                    if q == c {
                        break;
                    }
                    w.push(q);
                }
            },
            '\\' => {
                let w = cur.get_or_insert_with(String::new);
                w.extend(chars.next());
            },
            c => cur.get_or_insert_with(String::new).push(c),
        }
    }
    words.extend(cur);
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn run(args: &[&str], files: &[(&str, &str)]) -> Result<Expanded> {
        let files: HashMap<String, String> = files.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        expand(&s(args), &mut |f| Ok(files.get(f).cloned()))
    }

    #[test]
    fn split_handles_comments_quotes_and_escapes() {
        assert_eq!(split("## a comment\n+P 5 # trailing\n-kernel x \"a b\" 'c d' e\\ f\n"), s(&["+P", "5", "-kernel", "x", "a b", "c d", "e f"]));
    }

    #[test]
    fn plus_flags_become_emulator_flags_with_their_values() {
        let e =
            run(&["-boot", "b", "-args_file", "vm"], &[("vm", "+hmax 125000000 +zdbbl 100000 +sbwt none +S 4:4 +K true +Bi +B d +c false +e 5000")]).unwrap();
        assert_eq!(
            e.emu,
            s(&["-hmax", "125000000", "-zdbbl", "100000", "-sbwt", "none", "-S", "4:4", "-K", "true", "-Bi", "-B", "d", "-c", "false", "-e", "5000"])
        );
        assert_eq!(e.erl, s(&["-boot", "b"]));
    }

    #[test]
    fn flags_without_a_value_leave_the_next_word_alone() {
        let e = run(&["-args_file", "vm"], &[("vm", "+sbwtdcpu none +sbe true +Ktrue -kernel a b")]).unwrap();
        assert_eq!(e.emu, s(&["-sbwtdcpu", "none", "-sbe", "-Ktrue"]));
        assert_eq!(e.erl, s(&["true", "-kernel", "a", "b"]));
    }

    #[test]
    fn init_arguments_stay_where_the_file_was() {
        let e = run(&["-noshell", "-args_file", "vm", "-mode", "embedded"], &[("vm", "-kernel shell_history enabled")]).unwrap();
        assert_eq!(e.erl, s(&["-noshell", "-kernel", "shell_history", "enabled", "-mode", "embedded"]));
    }

    #[test]
    fn env_extra_and_nested_files() {
        let e = run(&["-args_file", "vm"], &[("vm", "-env ERL_FULLSWEEP_AFTER 10 -args_file more +A 4 -extra x y"), ("more", "+P 100")]).unwrap();
        assert_eq!(e.env, vec![("ERL_FULLSWEEP_AFTER".to_string(), "10".to_string())]);
        assert_eq!((e.emu, e.extra), (s(&["-P", "100", "-A", "4"]), s(&["x", "y"])));
    }

    #[test]
    fn distribution_flags_are_set_apart() {
        let e =
            run(&["-args_file", "vm"], &[("vm", "-name app@127.0.0.1 -setcookie secret -hidden -start_epmd false -erl_epmd_port 6789 -detached -kernel a b")])
                .unwrap();
        let some = |v: &str| Some(v.to_string());
        assert_eq!(
            e.dist,
            vec![
                ("-name".to_string(), some("app@127.0.0.1")),
                ("-setcookie".to_string(), some("secret")),
                ("-hidden".to_string(), None),
                ("-start_epmd".to_string(), some("false")),
                ("-erl_epmd_port".to_string(), some("6789"))
            ]
        );
        assert_eq!((e.dropped, e.erl), (s(&["-detached"]), s(&["-kernel", "a", "b"])));
    }

    #[test]
    fn added_text_goes_after_the_files_and_can_include_one() {
        let mut e = run(&["-args_file", "vm"], &[("vm", "+S 4:4"), ("more", "+A 2")]).unwrap();
        let files: HashMap<String, String> = [("more".to_string(), "+A 2".to_string())].into();
        add("--vm-args", "+S 2:2 -args_file more -kernel x y", &mut |f| Ok(files.get(f).cloned()), &mut e).unwrap();
        assert_eq!(e.emu, s(&["-S", "4:4", "-S", "2:2", "-A", "2"]));
        assert_eq!(e.erl, s(&["-kernel", "x", "y"]));
        assert!(add("--vm-args", "+P", &mut |_| Ok(None), &mut e).unwrap_err().to_string().contains("--vm-args: +P needs a value"));
    }

    #[test]
    fn relative_scheduler_counts_resolve_against_host_cores() {
        let mut emu = s(&["-S", ":6", "-SDcpu", "-2:-4", "-S", "0", "-S", "4:2", "-S12:3", "-SDcpu:2", "-SDio", "2", "-SP", "50:50"]);
        resolve_schedulers(&mut emu, 18);
        assert_eq!(emu, s(&["-S", "18:6", "-SDcpu", "16:14", "-S", "18", "-S", "4:2", "-S12:3", "-SDcpu18:2", "-SDio", "2", "-SP", "50:50"]));
    }

    #[test]
    fn relative_online_counts_never_exceed_a_small_hosts_total() {
        let mut emu = s(&["-S", ":6", "-SDcpu", ":6", "-S", "8:6"]);
        resolve_schedulers(&mut emu, 4);
        assert_eq!(emu, s(&["-S", "4:4", "-SDcpu", "4:4", "-S", "8:6"]));
    }

    #[test]
    fn a_missing_file_is_skipped_and_a_cycle_is_refused() {
        assert_eq!(run(&["-args_file", "nope"], &[]).unwrap(), Expanded::default());
        assert!(run(&["-args_file", "vm"], &[("vm", "-args_file vm")]).is_err());
        assert!(run(&["-args_file", "vm"], &[("vm", "+P")]).is_err());
    }
}
