defmodule Greeter.E2ETest do
  # One packed binary, started once and shared by every test: the tests talk to it over HTTP like
  # any client would.
  use ExUnit.Case, async: false

  setup_all do
    bin = PackedApp.pack!()
    app = PackedApp.start!(bin, ["start"])
    on_exit(fn -> PackedApp.stop(app) end)
    %{bin: bin, app: app}
  end

  test "serves HTML", %{app: app} do
    resp = Req.get!(PackedApp.url(app, "/"))
    assert resp.status == 200
    assert resp.body =~ "Hello from a single binary"
  end

  test "escapes path parameters in HTML", %{app: app} do
    resp = Req.get!(PackedApp.url(app, "/hello/%3Cscript%3E"))
    assert resp.body =~ "Hello, &lt;script&gt;!"
  end

  test "answers JSON, reporting Linux whatever the host is", %{app: app} do
    assert %{"os" => "unix/linux", "otp_release" => "28"} = Req.get!(PackedApp.url(app, "/api/system")).body
  end

  test "parses JSON bodies", %{app: app} do
    assert Req.post!(PackedApp.url(app, "/api/echo"), json: %{"hello" => "wasm"}).body == %{"hello" => "wasm"}
  end

  test "streams server-sent events", %{app: app} do
    resp =
      Req.get!(PackedApp.url(app, "/api/ticks"),
        into: fn {:data, chunk}, {req, resp} ->
          body = (resp.body || "") <> chunk
          resp = %{resp | body: body}
          if length(Regex.scan(~r/event: tick/, body)) == 3, do: {:halt, {req, resp}}, else: {:cont, {req, resp}}
        end
      )

    assert Regex.scan(~r/"n":(\d)/, resp.body, capture: :all_but_first) == [["1"], ["2"], ["3"]]
  end

  test "handles concurrent requests", %{app: app} do
    statuses =
      1..200
      |> Task.async_stream(fn i -> Req.get!(PackedApp.url(app, "/hello/#{i}")).status end, max_concurrency: 50)
      |> Enum.map(fn {:ok, status} -> status end)

    assert Enum.all?(statuses, &(&1 == 200))
  end

  test "unknown paths are 404", %{app: app} do
    assert Req.get!(PackedApp.url(app, "/nope")).status == 404
  end

  test "refuses ports outside [network] listen", %{bin: bin} do
    {out, status} = PackedApp.run(bin, ["eval", ":gen_tcp.listen(5555, []) |> IO.inspect()"])
    assert status == 0
    assert out =~ "{:error, :eacces}"
  end

  test "runs an allowed host program, which sees the host's OS", %{app: app} do
    {host, 0} = System.cmd("uname", ["-s"])
    assert Req.get!(PackedApp.url(app, "/api/host")).body == %{"kernel" => String.trim(host)}
  end

  test "System.cmd/3 and :os.cmd/1 return output and exit status", %{bin: bin} do
    code = """
    IO.inspect(System.cmd("sh", ["-c", "echo out; echo err >&2; exit 3"], stderr_to_stdout: true))
    IO.inspect(:os.cmd(~c"echo $((6 * 7))"))
    """

    {out, 0} = PackedApp.run(bin, ["eval", code])
    assert out =~ ~s({"out\\nerr\\n", 3})
    assert out =~ ~s(~c"42\\n")
  end

  test "System.pid/0 is the host pid of the running binary", %{bin: bin} do
    code = """
    {out, 0} = System.cmd("sh", ["-c", "ps -o command= -p \#{System.pid()}"])
    IO.puts("ps: " <> String.trim(out))
    """

    {out, 0} = PackedApp.run(bin, ["eval", code])
    assert out =~ ~r"ps: \S*#{Regex.escape(Path.basename(bin))} eval"
  end

  test "a port writes to the program's stdin and reads its stdout", %{bin: bin} do
    code = """
    port = Port.open({:spawn_executable, "/bin/sh"}, [:binary, :exit_status, args: ["-c", "read l; echo got $l"]])
    Port.command(port, "ping\\n")
    receive do {^port, {:data, d}} -> IO.inspect(d) after 5000 -> IO.puts("no data") end
    receive do {^port, {:exit_status, s}} -> IO.inspect(s, label: "status") after 5000 -> IO.puts("no exit") end
    """

    {out, 0} = PackedApp.run(bin, ["eval", code])
    assert out =~ ~s("got ping\\n")
    assert out =~ "status: 0"
  end

  test "a program runs in the host directory behind the guest's", %{bin: bin} do
    code = """
    File.write!("/tmp/note.txt", "from the guest")
    IO.inspect(System.cmd("sh", ["-c", "cat note.txt"], cd: "/tmp"))
    """

    {out, 0} = PackedApp.run(bin, ["eval", code])
    assert out =~ ~s({"from the guest", 0})
  end

  test "refuses programs outside [process] allow", %{bin: bin} do
    code = """
    IO.inspect(System.find_executable("sh"))
    IO.inspect(Port.open({:spawn, "true"}, []))
    """

    {out, _} = PackedApp.run(bin, ["--allow-exec", "uname", "eval", code])
    assert out =~ "nil"
    assert out =~ "eacces"
  end

  test "chmod sets the host file's mode, never setuid, and not on read-only paths", %{bin: bin} do
    code = """
    import Bitwise
    mode = fn p -> {:ok, s} = File.lstat(p); Integer.to_string(s.mode &&& 0o7777, 8) end
    {:ok, io} = File.open("/tmp/secret", [:write, :exclusive])
    :ok = File.chmod("/tmp/secret", 0o600)
    IO.binwrite(io, "x"); File.close(io)
    File.mkdir!("/tmp/private"); :ok = File.chmod("/tmp/private", 0o700)
    IO.puts("modes: \#{mode.("/tmp/secret")} \#{mode.("/tmp/private")}")
    :ok = File.chmod("/tmp/secret", 0o4755)
    IO.puts("setuid dropped: \#{mode.("/tmp/secret")}")
    IO.inspect(File.chmod("/app/releases/start_erl.data", 0o600), label: "in /app")
    """

    {out, 0} = PackedApp.run(bin, ["eval", code])
    assert out =~ "modes: 600 700"
    assert out =~ "setuid dropped: 755"
    assert out =~ "in /app: {:error, :erofs}"
  end

  test "unix sockets: a local listener and client round-trip, abstract names are refused", %{bin: bin} do
    code = """
    {:ok, l} = :gen_tcp.listen(0, [:binary, ifaddr: {:local, "/tmp/app.sock"}, active: false])
    Task.start(fn -> {:ok, s} = :gen_tcp.accept(l); {:ok, d} = :gen_tcp.recv(s, 0); :gen_tcp.send(s, "echo:" <> d) end)
    {:ok, c} = :gen_tcp.connect({:local, "/tmp/app.sock"}, 0, [:binary, active: false])
    :gen_tcp.send(c, "hi")
    IO.inspect(:gen_tcp.recv(c, 0, 5000), label: "round trip")
    IO.inspect(:inet.sockname(l), label: "sockname")
    IO.inspect(:gen_tcp.connect({:local, <<0, "abstract">>}, 0, []), label: "abstract")
    """

    {out, 0} = PackedApp.run(bin, ["eval", code])
    assert out =~ ~s(round trip: {:ok, "echo:hi"})
    assert out =~ ~s(sockname: {:ok, {:local, "/tmp/app.sock"}})
    assert out =~ "abstract: {:error, :eacces}"
  end

  test "raises the memory alarm, and restarts a VM that runs out of memory", %{bin: bin} do
    # First run: grow by 16 MiB every 400 ms until the 256M cap kills the VM. The marker in the
    # shared --tmp makes the restarted VM grow once, past the alarm, and release it. It holds
    # the memory 8 s: far below the alarm the watch looks only every 5 s, so a shorter spike
    # can come and go unseen.
    tmp = Path.join(System.tmp_dir!(), "greeter-oom-#{System.unique_integer([:positive])}")
    File.mkdir_p!(tmp)

    grow = ~S"""
    -eval 'spawn(fun() -> timer:sleep(1000), case file:read_file_info("/tmp/crashed") of {error, enoent} -> file:write_file("/tmp/crashed", <<>>), F = fun G(Acc) -> timer:sleep(400), G([binary:copy(<<1>>, 16#1000000) | Acc]) end, F([]); _ -> L = [binary:copy(<<1>>, 16#1000000) || _ <- lists:seq(1, 5)], timer:sleep(8000), io:format("released ~B~n", [length(L)]) end end)'
    """

    args = [
      "--tmp",
      tmp,
      "--memory-max",
      "256M",
      "--memory-alarm",
      "30",
      "--restart",
      "on-failure",
      "--vm-args",
      String.trim(grow),
      "start"
    ]

    app = PackedApp.start!(bin, args)

    log =
      Enum.reduce_while(1..60, "", fn _, _ ->
        Process.sleep(500)
        log = PackedApp.log(app)
        if log =~ "alarm system_memory_high_watermark cleared", do: {:halt, log}, else: {:cont, log}
      end)

    assert Req.get!(PackedApp.url(app, "/")).status == 200
    PackedApp.stop(app)
    File.rm_rf!(tmp)
    assert log =~ "alarm system_memory_high_watermark set"
    assert log =~ "VM failed (status 1); restarting in 1s"
    assert log =~ "alarm system_memory_high_watermark cleared"
  end

  test "stops cleanly on SIGTERM", %{bin: bin} do
    app = PackedApp.start!(bin, ["start"])
    {status, log} = PackedApp.stop(app)
    assert status == 0
    assert log =~ "SIGTERM received - shutting down"
  end
end
