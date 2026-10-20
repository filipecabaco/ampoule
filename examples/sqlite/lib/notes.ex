defmodule Notes do
  @moduledoc """
  A notes store on SQLite, through exqlite's C NIF, which `ampoule release` rebuilds for wasm.

  The database is a file at `NOTES_DB` (default `/data/notes.db`): mount a host directory at
  `/data` and the notes outlive the process, in a file any SQLite tool can open.
  """
  alias Exqlite.Sqlite3

  @doc "Adds a note; returns its id."
  def add(body) do
    with_db(fn db ->
      {:ok, st} = Sqlite3.prepare(db, "insert into notes (body) values (?1)")
      :ok = Sqlite3.bind(st, [body])
      :done = Sqlite3.step(db, st)
      :ok = Sqlite3.release(db, st)
      {:ok, id} = Sqlite3.last_insert_rowid(db)
      id
    end)
  end

  @doc "Every note, oldest first, as `{id, body}`."
  def all do
    with_db(fn db ->
      {:ok, st} = Sqlite3.prepare(db, "select id, body from notes order by id")
      {:ok, rows} = Sqlite3.fetch_all(db, st)
      :ok = Sqlite3.release(db, st)
      Enum.map(rows, &List.to_tuple/1)
    end)
  end

  @doc "The SQLite library version inside the binary."
  def sqlite_version do
    with_db(fn db ->
      {:ok, st} = Sqlite3.prepare(db, "select sqlite_version()")
      {:row, [version]} = Sqlite3.step(db, st)
      :ok = Sqlite3.release(db, st)
      version
    end)
  end

  defp with_db(fun) do
    {:ok, db} = Sqlite3.open(System.get_env("NOTES_DB", "/data/notes.db"))

    try do
      :ok = Sqlite3.execute(db, "create table if not exists notes (id integer primary key, body text not null)")
      fun.(db)
    after
      Sqlite3.close(db)
    end
  end
end
