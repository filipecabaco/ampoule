defmodule PackedApp do
  @moduledoc """
  Drives the app as `ampoule` packs it: builds the binary, starts it, talks to it from outside,
  and stops it the way a supervisor would (SIGTERM).

  The `ampoule` used is `$AMPOULE`, else the one on PATH.
  """

  @doc "Packs the project with `ampoule release` (MIX_ENV=prod) and returns the binary's path."
  def pack! do
    ampoule =
      System.get_env("AMPOULE") || System.find_executable("ampoule") ||
        raise "set AMPOULE to the ampoule binary"

    root = File.cwd!()
    {out, status} = System.cmd(ampoule, ["release"], cd: root, env: [{"MIX_ENV", "prod"}], stderr_to_stdout: true)
    if status != 0, do: raise("ampoule release failed (#{status}):\n#{out}")

    case Path.wildcard(Path.join(root, "_build/prod/ampoule/*")) |> Enum.reject(&String.ends_with?(&1, ".partial")) do
      [bin] -> bin
      found -> raise "expected one packed binary, found #{inspect(found)}"
    end
  end

  @doc "Runs the binary to completion (`eval`, `info`, a custom command); returns `{output, exit_status}`."
  def run(bin, args, env \\ []), do: System.cmd(bin, args, env: env, stderr_to_stdout: true)

  @doc """
  Starts a long-running command (`start`, `serve`) with the app's port mapped to a free host port,
  and waits until it accepts connections.

  Options: `:env` (a list of `{name, value}`), `:guest_port` (default 4000), `:timeout` (ms).
  """
  def start!(bin, args, opts \\ []) do
    port = free_port()
    log = Path.join(System.tmp_dir!(), "packed-app-#{System.unique_integer([:positive])}.log")
    env = for {k, v} <- Keyword.get(opts, :env, []), do: {String.to_charlist(k), String.to_charlist(v)}
    # Through sh so the binary's pid is known (it execs in place) and its output lands in a file.
    script = ~s(echo $$; exec "$0" --port-map #{Keyword.get(opts, :guest_port, 4000)}:#{port} "$@" > "#{log}" 2>&1)
    caller = self()

    # The port's owner outlives the process that started the app (ExUnit's setup_all), so any
    # test can stop it and read its exit status.
    holder =
      spawn(fn ->
        owner =
          Port.open({:spawn_executable, "/bin/sh"}, [:binary, :exit_status, args: ["-c", script, bin | args], env: env])

        receive do: ({^owner, {:data, pid}} -> send(caller, {:started, self(), String.trim(pid)}))
        hold(owner, nil, [])
      end)

    os_pid = receive do: ({:started, ^holder, pid} -> pid)
    app = %{holder: holder, os_pid: os_pid, port: port, log: log}
    wait_until_listening!(app, System.monotonic_time(:millisecond) + Keyword.get(opts, :timeout, 60_000))
    app
  end

  def url(app, path), do: "http://127.0.0.1:#{app.port}#{path}"

  def log(app), do: File.read!(app.log)

  @doc "Sends SIGTERM and waits for the process to exit; returns `{exit_status, log}`."
  def stop(app, timeout \\ 40_000) do
    System.cmd("kill", ["-TERM", app.os_pid])

    case exit_status(app, timeout) do
      nil ->
        System.cmd("kill", ["-KILL", app.os_pid])
        raise "still running #{div(timeout, 1000)}s after SIGTERM:\n#{log(app)}"

      status ->
        {status, log(app)}
    end
  end

  defp exit_status(app, timeout) do
    send(app.holder, {:await, self()})
    holder = app.holder
    receive do: ({:exited, ^holder, status} -> status), after: (timeout -> nil)
  end

  defp hold(owner, status, waiters) do
    receive do
      {^owner, {:data, _}} ->
        hold(owner, status, waiters)

      {^owner, {:exit_status, s}} ->
        Enum.each(waiters, &send(&1, {:exited, self(), s}))
        hold(owner, s, [])

      {:await, from} when status != nil ->
        send(from, {:exited, self(), status})
        hold(owner, status, waiters)

      {:await, from} ->
        hold(owner, status, [from | waiters])
    end
  end

  defp wait_until_listening!(app, deadline) do
    cond do
      (status = exit_status(app, 0)) != nil ->
        raise "exited with #{status} before listening:\n#{log(app)}"

      System.monotonic_time(:millisecond) > deadline ->
        raise "never started listening:\n#{log(app)}"

      true ->
        case :gen_tcp.connect(~c"127.0.0.1", app.port, [], 500) do
          {:ok, s} ->
            :gen_tcp.close(s)

          {:error, _} ->
            Process.sleep(250)
            wait_until_listening!(app, deadline)
        end
    end
  end

  defp free_port do
    {:ok, l} = :gen_tcp.listen(0, [])
    {:ok, port} = :inet.port(l)
    :gen_tcp.close(l)
    port
  end
end
