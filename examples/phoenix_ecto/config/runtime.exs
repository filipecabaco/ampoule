import Config

# Read when the release boots, so the packed binary takes its settings from the environment.
if config_env() == :prod do
  config :notes, Notes.Repo,
    url: System.fetch_env!("DATABASE_URL"),
    pool_size: String.to_integer(System.get_env("POOL_SIZE", "5"))

  config :notes, NotesWeb.Endpoint,
    http: [port: String.to_integer(System.get_env("PORT", "4000"))],
    secret_key_base: System.fetch_env!("SECRET_KEY_BASE"),
    server: System.get_env("PHX_SERVER") == "true"
else
  config :notes, Notes.Repo, url: System.get_env("DATABASE_URL", "ecto://postgres:postgres@localhost/notes_dev")

  config :notes, NotesWeb.Endpoint,
    http: [port: 4000],
    secret_key_base: String.duplicate("dev-only-secret-", 4),
    server: true
end
