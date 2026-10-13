defmodule Smoke.MixProject do
  use Mix.Project

  # The app scripts/smoke.exs packs and runs: one C NIF (eiconv, built for wasm at pack time), a
  # TCP server, and a stop hook that shows a signal reached the VM as a clean shutdown.
  def project do
    [app: :smoke, version: "0.1.0", elixir: "~> 1.18", start_permanent: Mix.env() == :prod, deps: deps()]
  end

  def application do
    [extra_applications: [:logger, :ssl], mod: {Smoke.Application, []}]
  end

  defp deps do
    [{:eiconv, "~> 1.0"}]
  end
end
