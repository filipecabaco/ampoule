# What the app sees

A packed app runs inside wasm, on an operating system the runtime implements. It sees a small
filesystem, host sockets behind a network policy, and only the host programs it is allowed to
start.

## Filesystem

The app sees only these paths:

| Guest path | Backed by |
|---|---|
| `/app` | the embedded release (read-only; files are decompressed when opened) |
| `/tmp` | a private host directory (`--tmp`, or `$TMPDIR/ampoule-<pid>`); its `.ampoule/`, the runtime's own files, is read-only |
| `/etc/hosts` | the host's file plus `--host` entries |
| `/etc/resolv.conf` | the host's |
| `/etc/ssl/certs/ca-certificates.crt` | the host's CA bundle (or `AMPOULE_CA_BUNDLE`) |
| `/dev/null` | the host's |
| `/proc` | generated in Linux's formats: `meminfo`, `loadavg`, `stat`, `uptime`, `cpuinfo`, `self/{stat,statm,status,limits,fd}` |
| anything from `--mount` or `[[mounts]]` | the host path, optionally read-only |
| each program in `[process] allow` | the host's, read-only, at the path it has on the host |

Symlinks are resolved inside the app's view, never the host's: a link to `/` in `/tmp` points at
the app's own root, and a link to `/etc/passwd` finds nothing unless a mount provides that path.
Host paths are opened one component at a time, so a link created while a lookup is running cannot
redirect it either. Mount points themselves cannot be removed, renamed or replaced.

Without `--tmp`, the runtime removes its private `/tmp` when it exits. On the next start it also
removes directories left by runs that were killed. A directory holding an `erl_crash.dump` is kept,
and its path is printed.

The guest reports itself as Linux (`:os.type()` is `{:unix, :linux}`) on every host, so OTP and NIFs
take their Linux code paths, including os_mon reading `/proc`.

## Network

Sockets are host sockets. Binds and connects go through the network policy (`--listen`,
`--port-map`, `--listen-host`, `--rewrite-connect`, `--deny-connect`, and `[network]` in
`ampoule.toml`), and everything else goes straight to the host. TLS uses OTP's `ssl` with the
OpenSSL 3.5 linked into the engine.

UDP broadcast and multicast work as with any OTP release: `add_membership` joins the group on the
host (an IPv6 link-local group by interface index), and a datagram to a link-local group names its
interface in the destination's `scope_id`. `:inet.getifaddrs/0` lists the host's interfaces. OTP's
`socket` and `net` modules are not in the engine.

Unix domain sockets (`{:local, path}` with `gen_tcp` and `gen_udp`) work at the app's own paths:
binding creates the socket in a directory the app may write (`/tmp`, a writable mount), and
connecting reaches a socket file the app can see, such as a host service mounted in with `--mount
/var/run/app:/run/app`. Nothing outside the mounts is reachable, and Linux's abstract names (a
leading NUL byte) are refused with `:eacces`. A socket path is limited to 104 bytes on macOS and
108 on Linux as the host sees it; on macOS that includes the private `/tmp`'s own path.

## Host programs

Ports, `System.cmd/3` and `:os.cmd/1` start host programs, but only the ones the app allows in
`[process] allow` (or `--allow-exec`, which replaces that list). Nothing on the host is allowed by
default. Programs the release ships in its own `priv/` are allowed (see below).

```toml
[process]
allow = ["git", "/bin/sh"]   # a name is looked up on PATH; "*" allows everything on PATH
```

```elixir
System.cmd("git", ["rev-parse", "HEAD"])      # {"1f3c...\n", 0}
:os.cmd(~c"date +%s")                          # needs /bin/sh, which :os.cmd runs
Port.open({:spawn_executable, "/bin/sh"}, [:binary, :exit_status, args: ["-c", "cat"]])
```

Each allowed program is also mounted read-only at its host path, so `System.find_executable/1`
finds it. Programs missing on the host are left out, and the app gets the same `:enoent` it would
get natively. A program that is not allowed fails with `:eacces`.

- The program runs on the host, outside the app's view of the filesystem. Its path and working
  directory (`cd:`) are translated from the app's paths: `cd: "/tmp"` runs it in the host directory
  behind `/tmp`. Arguments and environment pass through unchanged, except `HOME`, which points at
  the host directory behind the app's `/tmp`, or at the host's `HOME` with `home = "host"`. A path
  inside `/app` means nothing to a host program.
- Allowing `/bin/sh` (which `:os.cmd/1` and `{:spawn, Cmd}` ports need) allows whatever the shell
  can start. On busybox systems, allowing one applet allows all of them, since the app chooses
  `argv[0]`.
- An empty `FROM scratch` image has no programs to run, not even `/bin/sh`.
- Exit statuses, `os_pid`, `stderr_to_stdout`, `nouse_stdio` and `{:packet, N}` behave as on a
  native release.

### Programs in `priv/`

Programs the release ships in its own `priv/` directories run without being allowed: they are the
app's code. Start one by its path, as on a native release:

```elixir
System.cmd(Path.join(:code.priv_dir(:my_app), "bin/tool"), ["--version"])
Port.open({:spawn_executable, Path.join(:code.priv_dir(:my_app), "bin/tool")}, [:binary, :exit_status])
```

The release's files exist only inside the binary, so the first time a program from an app's
`priv/` starts, the runtime extracts that app's whole `priv/` to the private `/tmp`, under
`.ampoule/priv/<app>-<vsn>/`, with the same layout and modes. A program that finds files next to
itself through its own path finds them there. Each app's `priv/` is extracted once per run, and the
app sees the copy read-only, so it cannot change what runs.

- Only a file below `/app/lib/<app>-<vsn>/priv/` qualifies, after symlinks are resolved inside the
  release. A link that leaves the app's `priv/` is not followed when extracting.
- The program runs on the host like any other: the same rules for `cd:`, arguments and environment
  apply, and a `#!` script's interpreter is whatever the host has at that path.
- Packing keeps shell scripts (`.sh`) under `priv/`, where it drops them elsewhere. It also reads
  the header of each executable in `priv/` and warns when one is built for another OS or CPU than
  the target, without failing:
  `==> warning: lib/my_app-0.1.0/priv/bin/tool is a macOS arm64 program; it won't run on x86_64-linux`.
  Build such programs for each target you pack for, or ship a script.
- A shell command line (`:os.cmd/1`, `{:spawn, Cmd}`) goes to the host's `/bin/sh` as it is, and a
  path inside `/app` means nothing there: start priv programs with `System.cmd/3` or
  `{:spawn_executable, Path}`.
- `[process] priv = false` turns this off: then a program in `priv/` fails with `:eacces`, as any
  program that is not allowed does.

[`examples/ports`](../../examples/ports) runs host programs, a program from `priv/`, and a
supervised long-running port.
