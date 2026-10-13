#!/usr/bin/env elixir

# Renders the site's social card: assets/og.html screenshotted in headless Chrome at 1200x630,
# written to site/og.png. Run it on macOS so the fonts and rendering match what was committed.
#
#   elixir scripts/og.exs [--chrome PATH]
#
# Platforms cache og:image by URL, so after a redesign write a new file name (og-v2.png) and
# point the meta tags at it.

defmodule OG do
  @chrome "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

  def run(argv) do
    {opts, _} = OptionParser.parse!(argv, strict: [chrome: :string])
    root = Path.expand("..", __DIR__)
    chrome = Keyword.get(opts, :chrome, @chrome)
    source = Path.join(root, "assets/og.html")
    out = Path.join(root, "site/og.png")

    unless File.exists?(chrome), do: fail("no Chrome at #{chrome}; pass --chrome PATH")

    args = [
      "--headless=new",
      "--hide-scrollbars",
      "--force-device-scale-factor=1",
      "--allow-file-access-from-files",
      "--virtual-time-budget=3000",
      "--window-size=1200,630",
      "--screenshot=#{out}",
      "file://#{source}"
    ]

    case System.cmd(chrome, args, stderr_to_stdout: true) do
      {_, 0} -> :ok
      {output, code} -> fail("Chrome exited #{code}:\n#{output}")
    end

    {size, 0} = System.cmd("sips", ["-g", "pixelWidth", "-g", "pixelHeight", out])
    [w, h] = Regex.scan(~r/pixel(?:Width|Height): (\d+)/, size, capture: :all_but_first) |> List.flatten()
    unless {w, h} == {"1200", "630"}, do: fail("#{out} is #{w}x#{h}, not 1200x630")

    IO.puts("#{Path.relative_to(out, root)}: 1200x630, #{div(File.stat!(out).size, 1024)} kB")
  end

  defp fail(msg) do
    IO.puts(:stderr, "og: #{msg}")
    System.halt(1)
  end
end

OG.run(System.argv())
