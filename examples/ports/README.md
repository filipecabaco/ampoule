# Ports

An OTP app that starts host programs, as one binary: a one-shot `System.cmd/3`, an `:os.cmd/1`
pipeline, and a long-running port program owned by a supervised GenServer.

```sh
ampoule release
_build/prod/ampoule/porter-<target> host
# host: Darwin arm64
# guest: {:unix, :linux}
# words: 3
```

- `ampoule.toml`: `[process] allow` lists the only programs the app may start: `uname`, `/bin/sh`
  (for `:os.cmd/1` and the port's shell loop) and `kill`. Each is mounted read-only at its host path,
  so `System.find_executable/1` finds it. Anything else is not there for the app.
- `lib/porter/cli.ex`: the `host` command (`[commands]`), with `System.cmd/3` and `:os.cmd/1`.
- `lib/porter/upcase.ex`: a shell loop behind `Port.open/2` that upper-cases each line it reads.
  The GenServer owns the port; when the program exits, its supervisor starts both again.

The programs run on the host and see the host's files. Allowing `/bin/sh` allows whatever a shell
script runs.

## Tests

`mix test` packs the app and checks the binary (`test/e2e/porter_test.exs`): the `host` command's
output, the port answering line by line, the supervisor starting the port program again after
`kill`, and a program outside `[process] allow` not being found.
