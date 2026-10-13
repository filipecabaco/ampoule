defmodule Smoke.Server do
  @moduledoc "Answers each line with `ok <the line, converted to Latin-1 and back by the NIF>`."
  use Task, restart: :permanent

  def start_link(port), do: Task.start_link(__MODULE__, :listen, [port])

  def listen(port) do
    {:ok, socket} = :gen_tcp.listen(port, [:binary, packet: :line, active: false, reuseaddr: true])
    IO.puts("smoke: listening on #{port}")
    accept(socket)
  end

  defp accept(socket) do
    {:ok, client} = :gen_tcp.accept(socket)
    spawn(fn -> serve(client) end)
    accept(socket)
  end

  defp serve(client) do
    case :gen_tcp.recv(client, 0) do
      {:ok, line} ->
        latin1 = :iconv.convert("utf-8", "iso-8859-1", String.trim_trailing(line))
        :ok = :gen_tcp.send(client, "ok #{:iconv.convert("iso-8859-1", "utf-8", latin1)}\n")
        serve(client)

      {:error, _} ->
        :gen_tcp.close(client)
    end
  end
end
