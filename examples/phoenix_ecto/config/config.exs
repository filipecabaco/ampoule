import Config

config :notes, ecto_repos: [Notes.Repo]

config :notes, NotesWeb.Endpoint,
  adapter: Bandit.PhoenixAdapter,
  http: [ip: {0, 0, 0, 0}],
  render_errors: [formats: [json: NotesWeb.ErrorJSON], layout: false]

config :phoenix, :json_library, Jason
config :logger, level: :info
