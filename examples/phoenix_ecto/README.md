# Phoenix + Ecto

A [Phoenix](https://phoenixframework.org) JSON API storing notes in Postgres through
[Ecto](https://hex.pm/packages/ecto_sql), as one binary that migrates its own database.

```sh
mix deps.get
ampoule release
export DATABASE_URL=ecto://postgres:postgres@localhost/notes SECRET_KEY_BASE=$(openssl rand -base64 48)
_build/prod/ampoule/notes-<target> serve     # migrate, then start: http://localhost:4000/api/notes
```

- `ampoule.toml`: `serve = ["migrate", "start"]`, `migrate` as a custom command
  (`Notes.Release.migrate()` creates the database if needed, then migrates), and `PHX_SERVER=true`
  for `start`.
- `config/runtime.exs`: everything the release needs comes from the environment at boot.
- The API: `GET/POST /api/notes`, `GET/DELETE /api/notes/:id`, `GET /api/health`.

```sh
curl -X POST localhost:4000/api/notes -H 'content-type: application/json' -d '{"title":"hello"}'
```

## Tests

`mix test` packs the app and runs `serve` against a fresh database (`test/e2e/notes_test.exs`):
migrations on first start, create/list/show/delete, validation errors, 404s, the `migrate` command,
data shared with a second instance, and a clean stop on SIGTERM. The database is dropped afterwards.

Postgres comes from `E2E_DATABASE_URL`, a server URL without a database name (default
`ecto://postgres:postgres@127.0.0.1:5432`).
