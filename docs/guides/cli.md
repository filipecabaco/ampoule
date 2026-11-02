# A CLI

A packed binary can be a command-line tool that users download and run: `my_cli pull o11y/x`, with
no Erlang on their machine and no wrapper script. In `ampoule.toml`, `main` gives the app the
command line, `[[mounts]]` lets it see the user's files, and `home = "host"` lets it find their
configuration.

```toml
home = "host"
main = "MyCli.main"

[process]
allow = ["gh"]

[[mounts]]
host = "$PWD"

[[mounts]]
host = "~/.config/my_cli"
create = true
```

## `main`: the app's command line

With `main`, every argument goes to the app, `help`, `--help` and none at all included:

```elixir
defmodule MyCli do
  def main(["pull", repo]), do: MyCli.Pull.run(repo)
  def main(_), do: IO.puts("usage: my_cli pull REPO")
end
```

- A function name, `MyCli.main` (or `my_cli:main` in an Erlang release), is called directly, as an
  escript's `main` is. The Elixir CLI, tokenizer, parser and evaluator that `--eval` loads stay
  unloaded, and the binary starts sooner. With arity 1 the function gets the arguments, as
  binaries (strings in Erlang); with arity 0 it reads `System.argv()` (`init:get_plain_arguments()`).
- An exception is printed as Elixir prints one and exits with status 1. `at_exit` hooks run.
  Returning exits with status 0, and `System.halt/1` sets another.
- Any other `main`, such as `MyCli.main(:fast)`, is evaluated as a `[commands]` entry is.

The runtime's own commands and options move behind `--vm` as the first argument:

```sh
my_cli --vm info                               # what is packed, including `main`
my_cli --vm --mount ~/x:/x eval 'IO.puts(1)'   # options, then a command
my_cli --vm help                               # the runtime's usage
```

The `AMPOULE_*` variables apply as before, so `AMPOULE_MOUNTS=... my_cli pull x` works without
`--vm`. `serve` steps run behind `--vm`, so a binary with `main` can still `--vm start` as a
server.

Without `main`, a `[commands]` entry makes a CLI too, one word longer: `cli = "MyCli.main()"` runs
as `my_cli cli pull o11y/x`.

## `[[mounts]]`: the user's files

The app sees only its own files until something is mounted. `[[mounts]]` packs the mounts the CLI
always needs, and expands `~`, `$VAR` and `${VAR}` where the binary runs:

- `host = "$PWD"` mounts the directory the user runs the binary in, at the same path. The app
  starts in `/`, so call `File.cd(System.get_env("PWD"))` to make relative paths work.
- `create = true` creates a missing directory (mode `0700`) instead of failing.
- An unset variable is an error rather than an empty string.

`--mount` or `AMPOULE_MOUNTS` replaces the packed mounts, so the user keeps control:
`AMPOULE_MOUNTS= my_cli pull x` runs with no mounts.

## `home = "host"`: the user's configuration

By default the app's `HOME` is its private `/tmp`. With `home = "host"`, `HOME` and
`System.user_home/0` are the host's, so `~/.config/my_cli` means what the user expects. The home
directory itself is not mounted: only what `[[mounts]]` provides is visible.

## Host programs

A CLI often calls other tools. `[process] allow` lists them, and they run on the host with the
host's `HOME` when `home = "host"`, so `gh` finds its login without `GH_CONFIG_DIR`. See [Host
programs](../reference/sandbox.md#host-programs).

## Start time

A CLI starts a VM on every run. On an Apple silicon Mac, a packed app whose `main` returns at once
takes about 0.1 s. The first run after install takes longer, because it writes the decompressed
engine to the [engine cache](../reference/runtime.md#the-engine-cache).
