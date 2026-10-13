# Examples

Three small apps, each packed into a single binary by `ampoule`, each with an end-to-end suite
(`test/e2e`) that packs the app, runs the real binary and checks it from the outside.

| Example | Shows |
|---|---|
| [`phoenix_ecto`](phoenix_ecto) | Phoenix JSON API on Ecto/Postgres; `serve` creates and migrates the database, then starts |
| [`francis`](francis) | Francis: HTML, JSON, server-sent events, the network policy |
| [`rustler`](rustler) | A Rustler NIF in the project's own `native/` crate, rebuilt for wasm at pack time |

Run one:

```sh
cd examples/francis
mix deps.get
mix test                  # packs with `ampoule release`, runs the binary, tests it over HTTP
```

or all of them from the repository root: `mise run examples` (or `elixir scripts/examples.exs`).

The suites use `$AMPOULE` as the `ampoule` binary (default: the one on `PATH`). Every example
has the same `test/support/packed_app.ex`, a copy of [`_shared/packed_app.ex`](_shared/packed_app.ex),
so each example stands on its own; `scripts/examples.exs` checks the copies have not drifted.
