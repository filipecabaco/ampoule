defmodule Smoke.Spike do
  @moduledoc """
  A memory spike for scripts/smoke.exs, which watches the process's RSS from outside: `mib` MiB of
  binaries (random bytes from OpenSSL, which the host can't compress away and which take a moment
  to make rather than seconds in Erlang's :rand), held for a moment and then freed.
  Prints `spike: peak` while they are held and `spike: freed` once every process has been
  garbage collected, then stays up for `linger_ms` for the RSS to be read.
  """

  def run(mib, linger_ms) do
    parent = self()

    holder =
      spawn(fn ->
        bins = for _ <- 1..div(mib, 16), do: :crypto.strong_rand_bytes(16 * 1_048_576)
        send(parent, :held)
        receive do: (:drop -> length(bins))
      end)

    receive do: (:held -> :ok)
    IO.puts("spike: peak #{div(:erlang.memory(:total), 1_048_576)} MiB")
    Process.sleep(2_000)

    ref = Process.monitor(holder)
    send(holder, :drop)
    receive do: ({:DOWN, ^ref, _, _, _} -> :ok)
    for p <- Process.list(), do: :erlang.garbage_collect(p)
    IO.puts("spike: freed #{div(:erlang.memory(:total), 1_048_576)} MiB")
    Process.sleep(linger_ms)
  end
end
