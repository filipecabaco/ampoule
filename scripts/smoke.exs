#!/usr/bin/env elixir

# End-to-end check of an ampoule binary: pack test/fixture (an Elixir app with a C NIF), then run
# the packed app and check it from the inside and the outside.
#
#   elixir scripts/smoke.exs [--ampoule PATH] [--network] [--keep]
#
#   --ampoule PATH  the binary to test (default: runner/target/release/ampoule)
#   --network         also check an outbound TLS connection (needs internet access)
#   --keep            leave build/smoke in place
#
# Needs Elixir with the engine's OTP major on PATH, and wasi-sdk plus the kit for the NIF
# (AMPOULE_WASI_SDK / AMPOULE_KIT, default build/wasi-sdk and build/engine/kit).

defmodule Smoke do
  @root Path.expand("..", __DIR__)

  def run(argv) do
    {opts, _} = OptionParser.parse!(argv, strict: [ampoule: :string, network: :boolean, keep: :boolean])
    ampoule = Path.expand(Keyword.get(opts, :ampoule, Path.join(@root, "runner/target/release/ampoule")))
    File.exists?(ampoule) || fail("#{ampoule} not found: build it with `cargo build --release` in runner/")

    work = Path.join(@root, "build/smoke")
    File.rm_rf!(work)
    File.cp_r!(Path.join(@root, "test/fixture"), work)

    env = [
      {"MIX_ENV", "prod"},
      {"AMPOULE_WASI_SDK", System.get_env("AMPOULE_WASI_SDK", Path.join(@root, "build/wasi-sdk"))},
      {"AMPOULE_KIT", System.get_env("AMPOULE_KIT", Path.join(@root, "build/engine/kit"))}
    ]

    step("pack", fn ->
      sh!("mix", ["deps.get"], work, env)
      sh!(ampoule, ["release"], work, env)
    end)

    [app] = Path.wildcard(Path.join(work, "_build/prod/ampoule/smoke-*"))

    step("info lists the NIF", fn ->
      assert(sh!(app, ["info"], work, []) =~ ~r/nifs:\s+eiconv/, "no eiconv in `info`")
    end)

    step("checks inside the guest", fn ->
      out = sh!(app, ["eval", "Smoke.Checks.run(network: #{Keyword.get(opts, :network, false)})"], work, [])
      IO.write(out)
      assert(out =~ "smoke checks: ok", "a check failed")
    end)

    step("serves TCP and stops cleanly on SIGTERM", fn -> serve_and_stop(app, work) end)

    unless opts[:keep], do: File.rm_rf!(work)
    IO.puts(IO.ANSI.green() <> "smoke: all passed" <> IO.ANSI.reset())
  end

  defp serve_and_stop(app, work) do
    log = Path.join(work, "serve.log")
    host_port = free_port()

    # Through `sh` so the app's pid is known and its output goes to a file.
    port =
      Port.open({:spawn_executable, "/bin/sh"}, [
        :exit_status,
        args: ["-c", "echo $$; exec \"$0\" --port-map 4000:#{host_port} start > \"$1\" 2>&1", app, log],
        env: [{~c"SMOKE_PORT", ~c"4000"}]
      ])

    pid = receive do: ({^port, {:data, data}} -> data |> to_string() |> String.trim())

    {:ok, sock} = connect(host_port, 60)
    :ok = :gen_tcp.send(sock, "olá\n")
    {:ok, reply} = :gen_tcp.recv(sock, 0, 5_000)
    assert(reply == "ok olá\n", "unexpected reply #{inspect(reply)}")
    :gen_tcp.close(sock)

    guest_tmp = Path.join(System.tmp_dir!(), "ampoule-#{pid}")
    assert(File.dir?(guest_tmp), "no private /tmp at #{guest_tmp}")

    System.cmd("kill", ["-TERM", pid])
    status = receive do: ({^port, {:exit_status, s}} -> s), after: (35_000 -> fail("still running 35s after SIGTERM"))
    out = File.read!(log)
    assert(status == 0, "exit status #{status} after SIGTERM:\n#{out}")
    assert(out =~ "smoke: stopping cleanly", "the release did not stop through init:stop():\n#{out}")
    assert(not File.exists?(guest_tmp), "private /tmp #{guest_tmp} left behind")
  end

  defp connect(_port, 0), do: fail("the app never accepted connections")

  defp connect(port, tries) do
    case :gen_tcp.connect(~c"127.0.0.1", port, [:binary, packet: :line, active: false], 1_000) do
      {:ok, s} ->
        {:ok, s}

      {:error, _} ->
        Process.sleep(500)
        connect(port, tries - 1)
    end
  end

  defp free_port do
    {:ok, l} = :gen_tcp.listen(0, [])
    {:ok, port} = :inet.port(l)
    :gen_tcp.close(l)
    port
  end

  defp step(name, fun) do
    IO.puts(IO.ANSI.bright() <> "==> #{name}" <> IO.ANSI.reset())
    fun.()
  end

  defp sh!(bin, args, cd, env) do
    case System.cmd(bin, args, cd: cd, env: env, stderr_to_stdout: true) do
      {out, 0} -> out
      {out, status} -> fail("#{Path.basename(bin)} #{Enum.join(args, " ")} exited with #{status}\n#{out}")
    end
  end

  defp assert(true, _msg), do: :ok
  defp assert(_, msg), do: fail(msg)

  defp fail(msg) do
    IO.puts(:stderr, IO.ANSI.red() <> "smoke: " <> msg <> IO.ANSI.reset())
    System.halt(1)
  end
end

Smoke.run(System.argv())
