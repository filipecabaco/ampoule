defmodule Notes.MixProject do
  use Mix.Project

  def project do
    [
      app: :notes,
      version: "0.1.0",
      elixir: "~> 1.18",
      start_permanent: Mix.env() == :prod,
      elixirc_paths: if(Mix.env() == :test, do: ["lib", "test/support"], else: ["lib"]),
      deps: deps(),
      # The suite tests the packed binary, not the app inside `mix test`.
      aliases: [test: "test --no-start"]
    ]
  end

  def application do
    [extra_applications: [:logger], mod: {Notes.Application, []}]
  end

  defp deps do
    [
      {:phoenix, "~> 1.8"},
      {:ecto_sql, "~> 3.14"},
      {:postgrex, "~> 0.22"},
      {:bandit, "~> 1.12"},
      {:jason, "~> 1.4"},
      {:req, "~> 0.5", only: :test}
    ]
  end
end
