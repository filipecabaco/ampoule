# Francis

A [Francis](https://hex.pm/packages/francis) app as one binary: an HTML page, a JSON API and a
server-sent event stream.

```sh
mix deps.get
ampoule release
_build/prod/ampoule/greeter-<target> start          # http://localhost:4000
PORT=8080 _build/prod/ampoule/greeter-<target> start
```

- `lib/greeter.ex`: the routes. `start/2` reads `PORT` when the release boots: Francis's own
  `start/2` only takes compile-time Bandit options.
- `ampoule.toml`: `[network] listen = [4000]` allows binding port 4000 only. Run with
  `--port-map 4000:8080` to serve on host port 8080 instead. `[process] allow` lets the app start
  `uname` (for `/api/host`) and `/bin/sh`, and nothing else.

## Tests

`mix test` packs the app and checks the running binary (`test/e2e/greeter_test.exs`): HTML and
escaping, JSON, request bodies, server-sent events, 200 concurrent requests, a refused bind outside
`[network] listen`, host programs (`System.cmd/3`, `:os.cmd/1`, a port's stdin and stdout, `cd:`,
a program outside `[process] allow`), unix sockets, the memory alarm and a restart after running out
of memory, and a clean stop on SIGTERM.
