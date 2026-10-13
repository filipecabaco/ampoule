defmodule NotesWeb.Router do
  use Phoenix.Router

  pipeline :api do
    plug :accepts, ["json"]
  end

  scope "/api", NotesWeb do
    pipe_through :api

    get "/health", HealthController, :show
    resources "/notes", NoteController, only: [:index, :create, :show, :delete]
  end
end
