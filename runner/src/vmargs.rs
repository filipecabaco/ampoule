//! `-args_file`, applied the way erlexec applies it. The runtime starts beam directly, without
//! erlexec, so this is what reads a release's vm.args: `+` flags become emulator flags, `-env`
//! sets an environment variable, `-extra` starts plain arguments and everything else stays an
//! init argument where `-args_file` was. Distribution and host-process flags are dropped: the app
//! runs as one node and cannot start host programs. Splitting and the table of flags that take a
//! value follow erts/etc/common/erlexec.c.

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

fn include(file: &str, read: &mut dyn FnMut(&str) -> Result<Option<String>>, out: &mut Expanded, depth: usize) -> Result<()> {
    if depth >= MAX_DEPTH {
        bail!("{file}: -args_file nested more than {MAX_DEPTH} deep");
    }
    let Some(text) = read(file)? else { return Ok(()) };
    let words = split(&text);
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
            "-name" | "-sname" | "-setcookie" | "-proto_dist" | "-start_epmd" | "-epmd_module" | "-epmd" => {
                let v = take(1, "a value")?;
                out.dropped.push(format!("{w} {}", v[0]));
                i += 2;
            },
            "-hidden" | "-no_epmd" | "-detached" | "-heart" => {
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
    fn distribution_flags_are_dropped() {
        let e = run(&["-args_file", "vm"], &[("vm", "-name app@127.0.0.1 -setcookie secret -hidden -kernel a b")]).unwrap();
        assert_eq!(e.dropped, s(&["-name app@127.0.0.1", "-setcookie secret", "-hidden"]));
        assert_eq!(e.erl, s(&["-kernel", "a", "b"]));
    }

    #[test]
    fn a_missing_file_is_skipped_and_a_cycle_is_refused() {
        assert_eq!(run(&["-args_file", "nope"], &[]).unwrap(), Expanded::default());
        assert!(run(&["-args_file", "vm"], &[("vm", "-args_file vm")]).is_err());
        assert!(run(&["-args_file", "vm"], &[("vm", "+P")]).is_err());
    }
}
