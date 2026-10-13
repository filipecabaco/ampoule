defmodule Notes.E2ETest do
  # Packs the app, then runs `serve` (migrate, then start) against a fresh Postgres database and
  # uses the API over HTTP. Postgres comes from $E2E_DATABASE_URL (a server URL, no database name).
  use ExUnit.Case, async: false

  setup_all do
    server = System.get_env("E2E_DATABASE_URL", "ecto://postgres:postgres@127.0.0.1:5432")
    database_url = "#{server}/notes_e2e_#{System.unique_integer([:positive])}"
    env = [{"DATABASE_URL", database_url}, {"SECRET_KEY_BASE", Base.encode64(:crypto.strong_rand_bytes(48))}]

    bin = PackedApp.pack!()
    app = PackedApp.start!(bin, ["serve"], env: env)

    on_exit(fn ->
      PackedApp.stop(app)
      :ok = Ecto.Adapters.Postgres.storage_down(Ecto.Repo.Supervisor.parse_url(database_url))
    end)

    %{bin: bin, app: app, env: env}
  end

  defp api(app, path), do: PackedApp.url(app, "/api" <> path)

  test "serve created and migrated the database before starting", %{app: app} do
    assert %{"status" => "ok", "postgres" => _} = Req.get!(api(app, "/health")).body
    assert PackedApp.log(app) =~ "create table notes"
  end

  test "creates, lists, shows and deletes notes", %{app: app} do
    created = Req.post!(api(app, "/notes"), json: %{title: "Packed", body: "one binary"})
    assert created.status == 201
    %{"id" => id, "title" => "Packed"} = created.body["data"]

    assert Enum.any?(Req.get!(api(app, "/notes")).body["data"], &(&1["id"] == id))
    assert Req.get!(api(app, "/notes/#{id}")).body["data"]["body"] == "one binary"
    assert Req.delete!(api(app, "/notes/#{id}")).status == 204
    assert Req.get!(api(app, "/notes/#{id}"), retry: false).status == 404
  end

  test "rejects invalid notes with the changeset errors", %{app: app} do
    resp = Req.post!(api(app, "/notes"), json: %{body: "no title"})
    assert resp.status == 422
    assert resp.body == %{"errors" => %{"title" => ["can't be blank"]}}
  end

  test "unknown ids and routes are 404", %{app: app} do
    assert Req.get!(api(app, "/notes/999999"), retry: false).status == 404
    assert Req.get!(api(app, "/notes/abc"), retry: false).status == 404
    assert Req.get!(api(app, "/nope"), retry: false).status == 404
  end

  test "migrate is a custom command and is idempotent", %{bin: bin, env: env} do
    assert {_, 0} = PackedApp.run(bin, ["migrate"], env)
  end

  test "data outlives the process; a second instance stops cleanly on SIGTERM", %{bin: bin, app: app, env: env} do
    %{"id" => id} = Req.post!(api(app, "/notes"), json: %{title: "durable"}).body["data"]

    other = PackedApp.start!(bin, ["start"], env: env)
    assert Req.get!(api(other, "/notes/#{id}")).body["data"]["title"] == "durable"

    {status, log} = PackedApp.stop(other)
    assert status == 0
    assert log =~ "SIGTERM received - shutting down"
  end
end
