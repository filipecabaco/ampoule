defmodule Porter.CLI do
  @moduledoc "The `host` command (see ampoule.toml): one-shot host programs."

  def host do
    # System.cmd/3 finds uname through PATH, where ampoule mounts each allowed program.
    {kernel, 0} = System.cmd("uname", ["-sm"])
    IO.puts("host: " <> String.trim(kernel))
    # The app itself always reports Linux: it runs on ampoule's own OS layer.
    IO.puts("guest: #{inspect(:os.type())}")
    # :os.cmd/1 runs a shell pipeline through /bin/sh.
    words = :os.cmd(~c"printf 'one two three' | wc -w") |> to_string() |> String.trim()
    IO.puts("words: " <> words)
    IO.puts("pid: #{System.pid()}")
  end
end
