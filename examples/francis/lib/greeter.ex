defmodule Greeter do
  @moduledoc """
  A small Francis app: an HTML page, a JSON API and a server-sent event stream.
  """
  use Francis

  # Francis starts Bandit with compile-time options; read the port when the release boots instead,
  # so the packed binary honours PORT like any other release.
  @impl true
  def start(_type, _args) do
    port = String.to_integer(System.get_env("PORT", "4000"))
    Supervisor.start_link([{Bandit, plug: __MODULE__, port: port}], strategy: :one_for_one, name: Greeter.Supervisor)
  end

  get("/", fn conn ->
    html(conn, "<h1>Hello from a single binary</h1><p>Try <a href=\"/hello/you\">/hello/you</a>.</p>")
  end)

  get("/hello/:name", fn conn -> html(conn, "<h1>Hello, #{Francis.HTML.escape(conn.params["name"])}!</h1>") end)

  # What the app sees of the machine it runs on: the runtime reports Linux whatever the host is.
  get("/api/system", fn _conn ->
    {family, name} = :os.type()

    %{
      otp_release: to_string(:erlang.system_info(:otp_release)),
      os: "#{family}/#{name}",
      schedulers: System.schedulers_online()
    }
  end)

  post("/api/echo", fn conn -> conn.body_params end)

  # Three ticks, one every 100ms, sent from a process the handler starts on join.
  sse("/api/ticks", fn
    :join, socket ->
      transport = socket.transport

      spawn(fn ->
        for n <- 1..3 do
          Process.sleep(100)
          send(transport, {:tick, n})
        end
      end)

      :noreply

    {:received, {:tick, n}}, _socket ->
      {:reply, %{event: "tick", data: %{n: n}}}

    {:close, _reason}, _socket ->
      :ok
  end)

  unmatched(fn _conn -> "not found" end)
end
