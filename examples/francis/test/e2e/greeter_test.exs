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

  test "stops cleanly on SIGTERM", %{bin: bin} do
    app = PackedApp.start!(bin, ["start"])
    {status, log} = PackedApp.stop(app)
    assert status == 0
    assert log =~ "SIGTERM received - shutting down"
  end
end
