defmodule Greeter.MixProject do
  use Mix.Project

  def project do
    [
      app: :greeter,
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
    [extra_applications: [:logger], mod: {Greeter, []}]
  end

  defp deps do
    [
      {:francis, "~> 0.3.3"},
      {:req, "~> 0.5", only: :test}
    ]
  end
end
