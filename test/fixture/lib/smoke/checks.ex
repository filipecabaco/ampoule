defmodule Smoke.Checks do
  @moduledoc """
  What the runtime provides to the guest, checked from inside it. `run/1` prints one line per check
  and `smoke checks: ok` when all pass, or halts with status 1.
  """

  def run(opts \\ []) do
    checks = [
      {"reports Linux", fn -> :os.type() == {:unix, :linux} end},
      {"C NIF (eiconv) round-trip",
       fn -> :iconv.convert("iso-8859-1", "utf-8", :iconv.convert("utf-8", "iso-8859-1", "olá")) == "olá" end},
      {"schedulers run in parallel", fn -> 1..64 |> Task.async_stream(&(&1 * 2)) |> Enum.count() == 64 end},
      {"/tmp is writable", fn -> File.write!("/tmp/smoke", "x") == :ok and File.read!("/tmp/smoke") == "x" end},
      {"/app is read-only", fn -> match?({:error, _}, File.write("/app/smoke", "x")) end},
      {"/proc/meminfo is generated", fn -> File.read!("/proc/meminfo") =~ "MemTotal:" end},
      {"loopback TCP", &loopback/0},
      {"a symlink to / stays in the guest's root", &link_to_root/0},
      {"a symlink to a host file finds nothing",
       fn -> File.ln_s("/etc/passwd", "/tmp/pw") == :ok and File.read("/tmp/pw") == {:error, :enoent} end},
      {"mount points cannot be removed", fn -> File.rm("/dev/null") == {:error, :ebusy} end},
      {"crypto", fn -> byte_size(:crypto.hash(:sha256, "x")) == 32 end}
    ]

    checks =
      if Keyword.get(opts, :network, false), do: checks ++ [{"TLS client with peer verification", &tls/0}], else: checks

    failed =
      for {name, check} <- checks, not passes?(check) do
        name
      end

    for {name, _} <- checks, do: IO.puts("#{if name in failed, do: "FAIL", else: "ok  "} #{name}")
    if failed == [], do: IO.puts("smoke checks: ok"), else: System.halt(1)
  end

  defp passes?(check) do
    check.() == true
  rescue
    _ -> false
  end

  defp link_to_root do
    :ok = File.ln_s("/", "/tmp/root")
    Enum.sort(File.ls!("/tmp/root")) == Enum.sort(File.ls!("/"))
  end

  defp loopback do
    {:ok, l} = :gen_tcp.listen(0, [:binary, active: false])
    {:ok, port} = :inet.port(l)
    {:ok, c} = :gen_tcp.connect(~c"127.0.0.1", port, [:binary, active: false])
    {:ok, s} = :gen_tcp.accept(l)
    :ok = :gen_tcp.send(c, "ping")
    {:ok, "ping"} == :gen_tcp.recv(s, 4)
  end

  defp tls do
    {:ok, _} = Application.ensure_all_started(:ssl)
    host = ~c"hex.pm"

    opts = [
      verify: :verify_peer,
      cacerts: :public_key.cacerts_get(),
      server_name_indication: host,
      customize_hostname_check: [match_fun: :public_key.pkix_verify_hostname_match_fun(:https)]
    ]

    match?({:ok, _}, :ssl.connect(host, 443, opts, 15_000))
  end
end
