defmodule Porter.Greeter do
  @moduledoc """
  A program the app ships itself, in `priv/bin/greet`, started as it would be on a native release:
  by its path in the app's priv directory.

  It needs no `[process] allow` entry: programs in the release's own priv/ are the app's code.
  ampoule extracts the app's priv/ to its private /tmp the first time one of them starts, and runs
  it on the host from there.
  """

  @doc "The program's path, as the app sees it."
  def path, do: Path.join(:code.priv_dir(:porter), "bin/greet")

  @doc "One greeting, through `System.cmd/3`: `{output, exit_status}`."
  def greet(name, opts \\ []), do: System.cmd(path(), List.wrap(name), opts)

  @doc "A greeting per name, from the same program behind a port, read line by line."
  def greet_each(names) do
    port = Port.open({:spawn_executable, path()}, [:binary, :exit_status, {:line, 4096}, args: ["--lines"]])
    Enum.each(names, &Port.command(port, [&1, ?\n]))

    lines =
      for _ <- names do
        receive do
          {^port, {:data, {:eol, line}}} -> line
        after
          5_000 -> exit(:no_answer)
        end
      end

    Port.close(port)
    lines
  end
end
