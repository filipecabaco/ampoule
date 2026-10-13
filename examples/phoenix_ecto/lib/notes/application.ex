defmodule Notes.Application do
  @moduledoc false
  use Application

  @impl true
  def start(_type, _args) do
    Supervisor.start_link([Notes.Repo, NotesWeb.Endpoint], strategy: :one_for_one, name: Notes.Supervisor)
  end
end
