# SQLite

A notes store on SQLite, as one binary. SQLite comes from [exqlite](https://hex.pm/packages/exqlite),
a C NIF that `ampoule release` rebuilds for wasm from its sources: the binary carries its own
SQLite, and the machine that runs it needs none.

```sh
mix deps.get
ampoule release
mkdir -p data
_build/prod/ampoule/notes-<target> --mount "$PWD/data:/data" eval 'Notes.add("hello")'
_build/prod/ampoule/notes-<target> --mount "$PWD/data:/data" eval 'IO.inspect(Notes.all())'
sqlite3 data/notes.db 'select * from notes'      # a real SQLite file on the host
```

- `lib/notes.ex`: the store, on `Exqlite.Sqlite3`. The database is `NOTES_DB`, default
  `/data/notes.db`: mount a host directory at `/data` and the notes outlive the process.
- Packing forces exqlite to build from source (it would otherwise download a library built for
  the host), and under WASI SQLite runs without memory-mapped I/O and without loading extensions.

## Performance

Queries cost about 1.5× native SQLite. Transactions on a file database cost more: WAL is not
available under WASI, so each transaction takes the file locks and rereads the header, about
215 µs against 10 µs natively (Apple M5 Pro). For a database only this process uses, take the
lock once per connection:

```elixir
:ok = Exqlite.Sqlite3.execute(db, "PRAGMA locking_mode = EXCLUSIVE")
```

10,000 single-row reads, each its own transaction, then take 106 ms instead of 2.2 s. Batching
writes in one transaction helps the same way (10,000 inserts: 125 ms).

## Tests

`mix test` packs the app and checks the binary (`test/e2e/notes_test.exs`): notes written by one
run are read by the next, the file on the host is a SQLite database, and the binary reports its
own SQLite version.
