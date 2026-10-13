#!/usr/bin/env elixir

# Run Realtime's own e2e suite (test/e2e/realtime-check.ts, in the realtime repository) against
# Realtime packed with test/realtime/ampoule.toml (`ampoule release --config <this repo>/test/realtime/ampoule.toml`):
# a compatibility check on a large production app, not part of CI.
#
# Like `mise run e2e-local`, but instead of swapping in another container, the stack's realtime
# container is set aside and replaced by a forwarder (same name, network aliases) that sends
# Kong's traffic to the wasm Realtime running on the host. The wasm Realtime gets the original
# container's environment; the database names it uses resolve to 127.0.0.1 through --host entries, and
# the runner rewrites the container port to the published one.
#
#   elixir scripts/realtime.exs [--realtime ../realtime] [--binary REALTIME/_build/prod/ampoule/realtime-<target>] [--test functional,load] [--keep]
#   elixir scripts/realtime.exs --image realtime-wasm:linux     # run a Linux image of the binary on the stack's network instead

defmodule E2E do
  @forwarder_image "alpine/socat:1.8.0.3"
  @dropped_env ~w(PATH HOME HOSTNAME PWD SHLVL TERM ERL_AFLAGS ERL_FLAGS RELEASE_COOKIE RELEASE_NODE RELEASE_DISTRIBUTION
                  DNS_NODES CLUSTER_STRATEGIES NODE_NAME LANG LANGUAGE LC_ALL)

  def run(argv) do
    {opts, _} =
      OptionParser.parse!(argv,
        strict: [
          realtime: :string,
          binary: :string,
          image: :string,
          test: :string,
          keep: :boolean,
          port_offset: :integer
        ]
      )

    root = Path.expand("..", __DIR__)
    build = Path.join(root, "build")
    repo = Path.expand(Keyword.get(opts, :realtime, Path.join(root, "../realtime")))
    binary = Path.expand(Keyword.get(opts, :binary, Path.join(repo, "_build/prod/ampoule/realtime-#{host_target()}")))
    tests = Keyword.get(opts, :test, "functional,load")
    e2e = Path.join(repo, "test/e2e")
    stack = copy_stack(e2e, Path.join(build, "e2e-stack"), Keyword.get(opts, :port_offset, 200))

    ref = project_ref(stack)
    container = "supabase_realtime_#{ref}"
    native = "#{container}_native"

    sh!("supabase", ["start"], cd: stack)
    status = supabase_status(stack)
    db_port = status |> Map.fetch!("DB_URL") |> URI.parse() |> Map.fetch!(:port)

    unless running?(container) or running?(native),
      do: fail("#{container} is not running; `supabase stop && supabase start` in #{stack}")

    if not exists?(native), do: set_aside(container, native)
    env = container_env(native)

    if image = opts[:image] do
      run_image(image, container, ref, env, stack, e2e, status, db_port, tests, opts, build)
    end

    db_names =
      [env["DB_HOST"], "db", "supabase_db_#{ref}"]
      |> Enum.reject(&(&1 in [nil, "", "localhost", "127.0.0.1"]))
      |> Enum.uniq()

    start_forwarder(container, ref)

    flags =
      Enum.flat_map(db_names, &["--host", "#{&1}=127.0.0.1"]) ++
        ["--rewrite-connect", "127.0.0.1:5432=127.0.0.1:#{db_port}"]

    wasm_env = Map.put(env, "PORT", "4000") |> Enum.to_list()

    try do
      log = Path.join(build, "e2e-realtime.log")
      server = start_server(binary, flags, wasm_env, log)
      wait_healthy(log)

      {_, code} =
        System.cmd(
          "bun",
          [
            "run",
            "realtime-check.ts",
            "--env",
            "local",
            "--publishable-key",
            status["PUBLISHABLE_KEY"],
            "--secret-key",
            status["SECRET_KEY"],
            "--url",
            status["API_URL"],
            "--db-url",
            "postgresql://postgres:postgres@127.0.0.1:#{db_port}/postgres",
            "--test",
            tests
          ],
          cd: e2e,
          into: IO.stream(),
          stderr_to_stdout: true
        )

      stop_server(server)
      IO.puts("\nrealtime-check exited #{code} (wasm Realtime log: #{log})")
      code
    after
      unless opts[:keep], do: restore(container, native)
    end
    |> System.halt()
  end

  # The suite's supabase/ directory with every 54xxx port shifted and its own project id, so the
  # stack runs alongside any other local Supabase project.
  defp copy_stack(e2e, dir, offset) do
    File.rm_rf!(dir)
    File.mkdir_p!(dir)
    File.cp_r!(Path.join(e2e, "supabase"), Path.join(dir, "supabase"))
    config = Path.join(dir, "supabase/config.toml")

    config
    |> File.read!()
    |> String.replace(~r/^(\s*project_id\s*=\s*)"[^"]*"/m, ~s(\\1"e2e-wasm"))
    |> then(
      &Regex.replace(~r/^(\s*\w*port\s*=\s*)(54\d{3})\b/m, &1, fn _, k, p ->
        k <> Integer.to_string(String.to_integer(p) + offset)
      end)
    )
    |> then(&File.write!(config, &1))

    dir
  end

  defp project_ref(e2e) do
    e2e
    |> Path.join("supabase/config.toml")
    |> File.read!()
    |> then(&Regex.run(~r/^\s*project_id\s*=\s*"([^"]+)"/m, &1, capture: :all_but_first))
    |> case do
      [ref] -> ref
      _ -> fail("no project_id in #{e2e}/supabase/config.toml")
    end
  end

  defp supabase_status(e2e) do
    {out, 0} = System.cmd("supabase", ["status", "--output", "env"], cd: e2e, stderr_to_stdout: false)

    for line <- String.split(out, "\n"), [k, v] <- [String.split(line, "=", parts: 2)], into: %{} do
      {k, String.trim(v, "\"")}
    end
  end

  # Realtime from a Linux image, in place of the stack's own container: same name, network and
  # aliases, the native container's environment, and the stack's own DNS for the database.
  defp run_image(image, container, ref, env, _stack, e2e, status, db_port, tests, opts, build) do
    env_file = Path.join(build, "e2e-image.env")
    File.write!(env_file, Enum.map_join(Map.put(env, "PORT", "4000"), "", fn {k, v} -> "#{k}=#{v}\n" end))
    System.cmd("docker", ["rm", "-f", container], stderr_to_stdout: true)
    log = Path.join(build, "e2e-realtime.log")

    try do
      sh!("docker", [
        "run",
        "-d",
        "--name",
        container,
        "--network",
        "supabase_network_#{ref}",
        "--network-alias",
        "realtime",
        "--network-alias",
        "realtime-dev",
        "--env-file",
        env_file,
        image,
        "serve"
      ])

      wait_logged(container, "Running RealtimeWeb.Endpoint")
      Process.sleep(2000)

      {_, code} =
        System.cmd(
          "bun",
          [
            "run",
            "realtime-check.ts",
            "--env",
            "local",
            "--publishable-key",
            status["PUBLISHABLE_KEY"],
            "--secret-key",
            status["SECRET_KEY"],
            "--url",
            status["API_URL"],
            "--db-url",
            "postgresql://postgres:postgres@127.0.0.1:#{db_port}/postgres",
            "--test",
            tests
          ],
          cd: e2e,
          into: IO.stream(),
          stderr_to_stdout: true
        )

      {logs, _} = System.cmd("docker", ["logs", container], stderr_to_stdout: true)
      File.write!(log, logs)
      IO.puts("\nrealtime-check exited #{code} (image #{image}, log: #{log})")
      code
    after
      unless opts[:keep], do: restore(container, "#{container}_native")
    end
    |> System.halt()
  end

  defp wait_logged(container, line, tries \\ 120) do
    {logs, _} = System.cmd("docker", ["logs", container], stderr_to_stdout: true)

    cond do
      String.contains?(logs, line) ->
        IO.puts("#{container}: #{line}")

      tries > 0 and running?(container) ->
        Process.sleep(1000) && wait_logged(container, line, tries - 1)

      true ->
        IO.puts(logs |> String.split("\n") |> Enum.take(-40) |> Enum.join("\n")) &&
          fail("#{container} never logged #{inspect(line)}")
    end
  end

  defp exists?(name), do: match?({_, 0}, System.cmd("docker", ["inspect", name], stderr_to_stdout: true))

  defp running?(name) do
    case System.cmd("docker", ["inspect", "-f", "{{.State.Running}}", name], stderr_to_stdout: true) do
      {"true\n", 0} -> true
      _ -> false
    end
  end

  defp set_aside(container, native) do
    sh!("docker", ["rename", container, native])
    sh!("docker", ["stop", native])
  end

  defp container_env(native) do
    {out, 0} = System.cmd("docker", ["inspect", "-f", "{{range .Config.Env}}{{println .}}{{end}}", native])

    for line <- String.split(out, "\n"),
        [k, v] <- [String.split(line, "=", parts: 2)],
        k not in @dropped_env,
        into: %{} do
      {k, v}
    end
  end

  defp start_forwarder(container, ref) do
    System.cmd("docker", ["rm", "-f", container], stderr_to_stdout: true)

    sh!("docker", [
      "run",
      "-d",
      "--name",
      container,
      "--network",
      "supabase_network_#{ref}",
      "--network-alias",
      "realtime",
      "--network-alias",
      "realtime-dev",
      "--add-host",
      "host.docker.internal:host-gateway",
      @forwarder_image,
      "tcp-listen:4000,fork,reuseaddr",
      "tcp-connect:host.docker.internal:4000"
    ])
  end

  # `serve` runs migrate, seed (SEED_SELF_HOST=true) and start, like the image's run.sh.
  # ampoule's name for this machine, e.g. aarch64-darwin.
  defp host_target do
    arch = :erlang.system_info(:system_architecture) |> to_string() |> String.split("-") |> hd()
    os = if :os.type() == {:unix, :darwin}, do: "darwin", else: "linux"
    "#{arch}-#{os}"
  end

  defp start_server(binary, flags, env, log) do
    File.rm(log)
    charlist_env = Enum.map(env, fn {k, v} -> {String.to_charlist(k), String.to_charlist(v)} end)
    cmd = String.to_charlist(Enum.join([binary | flags] ++ ["serve"], " ") <> " >> #{log} 2>&1")
    Port.open({:spawn, cmd}, [:exit_status, env: charlist_env])
  end

  defp stop_server(port) do
    {:os_pid, pid} = Port.info(port, :os_pid)
    System.cmd("kill", [to_string(pid)])
  end

  defp wait_healthy(log, tries \\ 120) do
    case System.cmd("curl", ["-sSf", "-o", "/dev/null", "-H", "Host: realtime-dev", "http://127.0.0.1:4000/api/ping"],
           stderr_to_stdout: true
         ) do
      {_, 0} ->
        IO.puts("wasm Realtime is up")

      _ when tries > 0 ->
        Process.sleep(1000)
        wait_healthy(log, tries - 1)

      _ ->
        IO.puts(File.read!(log) |> String.split("\n") |> Enum.take(-40) |> Enum.join("\n"))
        fail("wasm Realtime did not answer /api/ping within 120s")
    end
  end

  defp restore(container, native) do
    if exists?(native), do: System.cmd("docker", ["rm", "-f", container], stderr_to_stdout: true)

    if match?({_, 0}, System.cmd("docker", ["inspect", native], stderr_to_stdout: true)) do
      sh!("docker", ["rename", native, container])
      sh!("docker", ["start", container])
    end
  end

  defp sh!(cmd, args, opts \\ []) do
    IO.puts(IO.ANSI.faint() <> "$ #{cmd} #{Enum.join(args, " ")}" <> IO.ANSI.reset())

    case System.cmd(cmd, args, Keyword.merge([into: IO.stream(), stderr_to_stdout: true], opts)) do
      {_, 0} -> :ok
      {_, code} -> fail("#{cmd} exited #{code}")
    end
  end

  defp fail(msg) do
    IO.puts(:stderr, "e2e: #{msg}")
    System.halt(1)
  end
end

E2E.run(System.argv())
