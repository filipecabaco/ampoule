# Configuration: `ampoule.toml`

`ampoule.toml` is optional. `ampoule release` reads it from the project root (or from `--config
FILE`) and stores it in the binary, so the machine that runs the app needs no config file. Packing
rejects unknown keys, a `[commands]` entry that shadows a built-in command, and `serve` steps that
are neither `start` nor a `[commands]` entry.

Every key, with an example value:

```toml
serve = ["migrate", { run = "seed", when = "SEED=true" }, "start"]   # `<binary> serve`
vm_args = "+S :4 +SDio 2"        # VM flags as in vm.args, after the release's own
idle_stop = "5m"                 # stop the VM after 5 minutes without connections
restart = "on-failure"           # start the VM again when it fails (a crash, out of memory)
memory_alarm = 80                # memory alarm at this % of the VM's memory limit (0: off)
native_cache = "auto"            # keep the compiled engine between starts: "auto", "off", or a directory
home = "host"                    # the app's home is the host's HOME (default: "private", its /tmp)
main = "MyCli.main"              # `<binary> ARGS...` runs the app's own command line

[commands]                       # `<binary> migrate` evaluates the expression
migrate = "MyApp.Release.migrate()"
seed = "MyApp.Release.seed()"

[env]                            # defaults for every command; the real environment wins
LANG = "C.UTF-8"

[start.env]                      # defaults for `start` only
PHX_SERVER = "true"

[network]
listen = [4000]                  # the only ports the app may bind (default: any)
port_map = { 4000 = 8080 }       # app port -> host port

[process]
allow = ["git", "/bin/sh"]       # host programs the app may start (default: none; "*": all on PATH)
priv = false                     # programs in the release's own priv/ may not run (default: they may)

[distribution]                   # `start` as a distributed node (default: off)
node = "my_app@10.0.0.5"         # NAME or NAME@HOST; RELEASE_NODE and --node win
mode = "name"                    # "name", "sname" or "none" (default: from `node`)
port = 9100                      # the distribution port
epmd = "none"                    # "none": peers are on `port`; "host": the host's epmd
cookie_file = "/run/secrets/erlang-cookie"

[[mounts]]                       # host paths the app sees
host = "$PWD"

[[mounts]]
host = "~/.config/my_app"
guest = "/config"                # default: the expanded host path
ro = false                       # read-only (default: false)
create = true                    # create the host directory, mode 0700, when it is missing
```

Most keys have a run option that overrides them where the binary runs; see [Running the packed
app](running.md#options).

## `serve`

`<binary> serve` runs these steps in order. Each step runs as its own VM, because a guest exit ends
the runtime process, and the last step replaces the process. A step is a command name, or
`{ run = "...", when = "..." }`, where `when` is `VAR` (set and non-empty) or `VAR=value`. Without
`serve`, `<binary> serve` is `start`.

## `[commands]`

Each entry is a command of the packed binary: `<binary> migrate` evaluates `MyApp.Release.migrate()`
on a release that is not started, as `bin/my_app eval` would. The arguments after the command name
are the expression's `System.argv()` (`init:get_plain_arguments()` without Elixir). A name may not
be one of the built-in commands (`start`, `serve`, `eval`, `remote`, `rpc`, `vm`, `info`, `help`).

## `main`

With `main`, the binary is a CLI that owns its whole command line: `my_cli pull o11y/x` runs `main`
with `["pull", "o11y/x"]`. The runtime's own commands and options move behind `--vm` as the first
argument (`my_cli --vm info`). A function name (`MyCli.main`, or `my_cli:main` in an Erlang release)
is called directly; any other value is evaluated as a `[commands]` entry is. See [Packing a
CLI](../guides/cli.md) for the details.

## `[env]` and `[start.env]`

Defaults for the app's environment: `[env]` for every command, `[start.env]` for `start` only. A
variable set in the real environment wins over both.

## `[network]`

`listen` is the list of ports the app may bind; without it the app may bind any port. `port_map`
binds an app port as a different host port. The [network policy](sandbox.md#network) has the rest.

## `[process]`

`allow` lists the host programs the app may start, by name (looked up on `PATH`) or by path; `"*"`
allows everything on `PATH`. Programs in the release's own `priv/` run without an entry, unless
`priv = false`. See [Host programs](sandbox.md#host-programs).

## `[distribution]`

Runs `start` as a distributed Erlang node. See [Clustering](../guides/cluster.md).

## `[[mounts]]` and `home`

`[[mounts]]` packs the host paths an app always needs, so its users pass no `--mount`:

- `~` (alone or before `/`), `$VAR` and `${VAR}` expand where the binary runs, not where it is
  packed: `$PWD` is the directory the user runs the binary in. An unset variable is an error, not an
  empty string, so `$UNSET/data` never mounts `/data`.
- Without `guest`, the app sees the host path at the same path. The app starts in `/`; it can call
  `File.cd(System.get_env("PWD"))`.
- A missing host path is an error, unless `create = true`: then the directory and its missing
  parents are created with mode `0700`.
- `--mount` or `AMPOULE_MOUNTS` replaces the packed mounts, as `--allow-exec` replaces `[process]
  allow`. `AMPOULE_MOUNTS=` runs the binary with no mounts. `info` lists the packed mounts.

`home = "host"` sets the app's `HOME`, and init's `-home` (`System.user_home/0`), to the host's
`HOME`. The app then finds `~/.config/my_cli` at the path the user knows. The home directory itself
is not mounted: only what `[[mounts]]` or `--mount` provide is visible. [Host
programs](sandbox.md#host-programs) get the host's `HOME` too, so `gh` or `git` find their
configuration. The default, `home = "private"`, makes `HOME` the app's private `/tmp`.

## `vm_args`

VM flags written as in `vm.args`. They apply after the release's own `vm.args` and before
`--vm-args`; see [VM flags](running.md#vm-flags).

## `idle_stop`, `restart`, `memory_alarm`, `native_cache`

These tune `start`: see [Stopping when idle](runtime.md#stopping-when-idle), [Memory and
restarts](runtime.md#memory-and-restarts) and [The engine cache](runtime.md#the-engine-cache).
