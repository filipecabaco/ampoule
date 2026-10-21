defmodule Roster.E2ETest do
  # Packs the app once and starts two nodes from the binary, a and b, without epmd. Both listen on
  # the distribution port 9100 inside, mapped to two free host ports (--port-map); a reaches b's
  # 9100 through --rewrite-connect, the way two machines would each have their own 9100.
  use ExUnit.Case, async: false

  @a :"a@127.0.0.1"
  @b :"b@127.0.0.1"

  setup_all do
    bin = PackedApp.pack!()
    cookie = "roster-e2e-#{System.unique_integer([:positive])}"
    env = [{"RELEASE_COOKIE", cookie}]
    b = PackedApp.start!(bin, ["--node", "#{@b}", "start"], guest_port: 9100, env: env)

    # --listen leaves out the distribution port; the runtime allows it all the same.
    a_args = ["--node", "#{@a}", "--listen", "4000", "--rewrite-connect", "127.0.0.1:9100=127.0.0.1:#{b.port}", "start"]
    a = PackedApp.start!(bin, a_args, guest_port: 9100, env: [{"ROSTER_PEERS", "#{@b}"} | env])

    on_exit(fn ->
      PackedApp.stop(a)
      PackedApp.stop(b)
    end)

    %{bin: bin, a: a, b: b, env: env, cookie: cookie}
  end

  # `rpc` on a node: a hidden node from the same binary, connecting to the node's host port.
  defp rpc(bin, node, app, expr, env, opts \\ []) do
    PackedApp.run(bin, opts ++ ["--node", "#{node}", "--dist-port", "#{app.port}", "rpc", expr], env)
  end

  defp eventually(fun, tries \\ 40) do
    case fun.() do
      {:ok, v} -> v
      {:error, _} when tries > 0 -> Process.sleep(500) && eventually(fun, tries - 1)
      {:error, last} -> flunk("never happened; last: #{inspect(last)}")
    end
  end

  test "info shows the distribution settings", %{bin: bin} do
    {out, 0} = PackedApp.run(bin, ["info"])
    assert out =~ "dist:      -name roster (port 9100, no epmd)"
  end

  test "the nodes connect, and each sees the other's member", %{bin: bin, a: a, b: b, env: env} do
    for {node, app} <- [{@a, a}, {@b, b}] do
      eventually(fn ->
        case rpc(bin, node, app, "IO.inspect(Roster.members())", env) do
          {out, 0} -> if out =~ ~s([:"a@127.0.0.1", :"b@127.0.0.1"]), do: {:ok, out}, else: {:error, out}
          other -> {:error, other}
        end
      end)
    end
  end

  test "calls go both ways", %{bin: bin, a: a, b: b, env: env} do
    eventually(fn ->
      if elem(rpc(bin, @a, a, "IO.inspect(Node.list())", env), 0) =~ "b@", do: {:ok, nil}, else: {:error, nil}
    end)

    {out, 0} = rpc(bin, @a, a, ~s|IO.inspect(:erpc.call(:"#{@b}", Roster, :roll_call, []))|, env)
    assert out =~ ~s([here: :"a@127.0.0.1", here: :"b@127.0.0.1"])
    {out, 0} = rpc(bin, @b, b, ~s|IO.inspect(:erpc.call(:"#{@a}", :erlang, :node, []))|, env)
    assert out =~ ~s(:"a@127.0.0.1")
  end

  test "rpc runs where the node is, with its environment", %{bin: bin, a: a, env: env} do
    {out, 0} =
      rpc(bin, @a, a, ~s|IO.inspect({node(), System.get_env("RELEASE_NODE"), System.get_env("RELEASE_COOKIE")})|, env)

    assert out =~ ~s({:"a@127.0.0.1", "a@127.0.0.1", nil})
  end

  test "a native node joins with the same cookie, and the packed node calls it back", %{a: a, cookie: cookie} do
    # This test's own VM: hidden, not listening, reaching a on its mapped port.
    Application.put_env(:kernel, :erl_epmd_node_listen_port, a.port)
    {:ok, _} = :net_kernel.start(:"native@127.0.0.1", %{name_domain: :longnames, dist_listen: false, hidden: true})

    try do
      Node.set_cookie(String.to_atom(cookie))
      assert Node.connect(@a)
      assert :erpc.call(@a, Roster, :members, []) |> Enum.member?(@a)
      otp = :erlang.system_info(:otp_release)
      assert :erpc.call(@a, :erpc, :call, [node(), :erlang, :system_info, [:otp_release]]) == otp
    after
      :net_kernel.stop()
      Application.delete_env(:kernel, :erl_epmd_node_listen_port)
    end
  end

  test "the cookie comes from a file as well, and a wrong one is refused", %{bin: bin, a: a, cookie: cookie} do
    file = Path.join(System.tmp_dir!(), "roster-cookie-#{System.unique_integer([:positive])}")
    File.write!(file, cookie <> "\n")

    try do
      {out, 0} = rpc(bin, @a, a, "IO.inspect(node())", [], ["--cookie-file", file])
      assert out =~ ~s(:"a@127.0.0.1")
    after
      File.rm(file)
    end

    {out, status} = rpc(bin, @a, a, "IO.inspect(node())", [{"RELEASE_COOKIE", "wrong"}])
    assert status != 0
    assert out =~ "noconnection"
    refute out =~ cookie
  end

  test "remote opens a shell on a running node", %{bin: bin, env: env} do
    # A node of its own: a remote shell whose input ends stops the node it is attached to, as
    # with a native release (so `stop` below finds it gone, and `kill` says so).
    c = PackedApp.start!(bin, ["--node", "c@127.0.0.1", "start"], guest_port: 9100, env: env)

    try do
      shell = "printf 'Roster.members()\\n' | \"$0\" --node c@127.0.0.1 --dist-port #{c.port} remote"
      {out, _} = System.cmd("/bin/sh", ["-c", shell, bin], env: env, stderr_to_stdout: true)
      assert out =~ "iex(c@127.0.0.1)1>"
      assert out =~ ~s([:"c@127.0.0.1"])
    after
      PackedApp.stop(c)
    end
  end
end
