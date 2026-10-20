defmodule Porter.E2ETest do
  # Packs the app once and runs the binary: its `host` command, and `eval`s that start the app.
  use ExUnit.Case, async: false

  setup_all do
    %{bin: PackedApp.pack!()}
  end

  test "host runs one-shot host programs: System.cmd/3 and :os.cmd/1", %{bin: bin} do
    {host, 0} = System.cmd("uname", ["-sm"])
    {out, 0} = PackedApp.run(bin, ["host"])
    assert out =~ "host: " <> String.trim(host)
    assert out =~ "guest: {:unix, :linux}"
    assert out =~ "words: 3"
    # System.pid/0 is the binary's own host pid.
    [_, pid] = Regex.run(~r/pid: (\d+)/, out)
    assert String.to_integer(pid) > 0
  end

  test "a long-running port program answers line by line", %{bin: bin} do
    code = """
    {:ok, _} = Application.ensure_all_started(:porter)
    IO.puts(Porter.Upcase.upcase("hello from the port"))
    IO.puts(Porter.Upcase.upcase("second line"))
    """

    {out, 0} = PackedApp.run(bin, ["eval", code])
    assert out =~ "HELLO FROM THE PORT"
    assert out =~ "SECOND LINE"
  end

  test "when the port program dies, the supervisor starts it again", %{bin: bin} do
    code = """
    {:ok, _} = Application.ensure_all_started(:porter)
    before = Porter.Upcase.os_pid()
    {_, 0} = System.cmd("kill", [Integer.to_string(before)])
    wait = fn wait ->
      Process.sleep(50)
      case Process.whereis(Porter.Upcase) do
        pid when is_pid(pid) -> if Porter.Upcase.os_pid() != before, do: :ok, else: wait.(wait)
        nil -> wait.(wait)
      end
    end
    wait.(wait)
    IO.puts("restarted: \#{Porter.Upcase.os_pid() != before}")
    IO.puts(Porter.Upcase.upcase("still here"))
    """

    {out, 0} = PackedApp.run(bin, ["eval", code])
    assert out =~ "restarted: true"
    assert out =~ "STILL HERE"
  end

  test "a program outside [process] allow is not there for the app", %{bin: bin} do
    code = """
    IO.inspect(System.find_executable("ls"), label: "ls")
    IO.inspect(Port.open({:spawn_executable, "/bin/ls"}, []))
    """

    {out, _} = PackedApp.run(bin, ["eval", code])
    assert out =~ "ls: nil"
    assert out =~ "enoent"
  end
end
