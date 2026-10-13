defmodule NotesWeb.HealthController do
  use Phoenix.Controller, formats: [:json]

  def show(conn, _params) do
    %{rows: [[version]]} = Notes.Repo.query!("select current_setting('server_version')")
    json(conn, %{status: "ok", postgres: version})
  end
end
