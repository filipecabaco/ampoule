defmodule Porter.MixProject do
  use Mix.Project

  def project do
    [
      app: :porter,
      version: "0.1.0",
      elixir: "~> 1.18",
      start_permanent: Mix.env() == :prod,
      elixirc_paths: if(Mix.env() == :test, do: ["lib", "test/support"], else: ["lib"]),
      deps: [],
      # The suite tests the packed binary, not the app inside `mix test`.
      aliases: [test: "test --no-start"]
    ]
  end

  def application do
    [extra_applications: [:logger], mod: {Porter.Application, []}]
  end
end
