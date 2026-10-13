defmodule Notes.Release do
  @moduledoc """
  Run from the packed binary before the app starts (see `serve` in ampoule.toml): creates the
  database if it does not exist yet, then runs pending migrations.
  """

  @app :notes

  def migrate do
    Application.load(@app)

    for repo <- Application.fetch_env!(@app, :ecto_repos) do
      case repo.__adapter__().storage_up(repo.config()) do
        :ok -> :ok
        {:error, :already_up} -> :ok
        {:error, reason} -> raise "could not create the database: #{inspect(reason)}"
      end

      {:ok, _, _} = Ecto.Migrator.with_repo(repo, &Ecto.Migrator.run(&1, :up, all: true))
    end
  end
end
