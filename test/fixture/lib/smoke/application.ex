defmodule Smoke.Application do
  @moduledoc false
  use Application

  @impl true
  def start(_type, _args) do
    port = String.to_integer(System.get_env("SMOKE_PORT", "4000"))
    Supervisor.start_link([{Smoke.Server, port}], strategy: :one_for_one, name: Smoke.Supervisor)
  end

  # Runs only when the release shuts down through init:stop(), not when the process is killed.
  @impl true
  def prep_stop(state) do
    IO.puts("smoke: stopping cleanly")
    state
  end
end
