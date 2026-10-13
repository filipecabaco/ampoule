# Generates the ampoule mark: the wasm magic bytes 00 61 73 6D as a 2x2 grid of tiles, each
# byte's hex digits drawn as seven-segment glyphs (no font needed).
#   assets/logo.svg        full colour
#   assets/logo-small.svg  tiles only, for 16px and below
#   site/favicon.svg       full colour (same as logo.svg)
#   stdout                 the one-ink <symbol id="mark"> for site/index.html (paste it there)
#
#   elixir scripts/logo.exs
defmodule Logo do
  @segments %{
    a: {{2, 0}, {14, 0}},
    b: {{16, 2}, {16, 12}},
    c: {{16, 16}, {16, 26}},
    d: {{2, 28}, {14, 28}},
    e: {{0, 16}, {0, 26}},
    f: {{0, 2}, {0, 12}},
    g: {{2, 14}, {14, 14}}
  }
  # Standard seven-segment hex forms (lowercase d for D, as on hex displays).
  @digits %{
    "0" => ~w(a b c d e f)a,
    "1" => ~w(b c)a,
    "3" => ~w(a b c d g)a,
    "6" => ~w(a c d e f g)a,
    "7" => ~w(a b c)a,
    "D" => ~w(b c d e g)a
  }
  @tiles [
    {{3, 3}, ~w(0 0), "#f2c14e", "#17181a"},
    {{67, 3}, ~w(6 1), "#7a3a0b", "#ffe7a8"},
    {{3, 67}, ~w(7 3), "#7a3a0b", "#ffe7a8"},
    {{67, 67}, ~w(6 D), "#17181a", "#f2c14e"}
  ]

  defp digit(char, x, y) do
    @digits
    |> Map.fetch!(char)
    |> Enum.map_join(" ", fn seg ->
      {{x1, y1}, {x2, y2}} = Map.fetch!(@segments, seg)
      "M#{x + x1} #{y + y1}L#{x + x2} #{y + y2}"
    end)
  end

  defp digits({x, y}, [d1, d2]), do: "#{digit(d1, x + 10, y + 15)} #{digit(d2, x + 32, y + 15)}"

  defp rect({x, y}, fill), do: ~s(<rect x="#{x}" y="#{y}" width="58" height="58" rx="9" fill="#{fill}"/>)

  defp head(comment) do
    ~s(<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 128 128" width="128" height="128" role="img" aria-labelledby="title">\n) <>
      ~s(  <title id="title">ampoule</title>\n) <> "  <!-- #{comment} -->\n"
  end

  def full do
    tiles =
      Enum.map_join(@tiles, "", fn {pos, ds, fill, ink} ->
        # The ink tile carries a gold rim so its edge holds on dark backgrounds too.
        rim =
          if fill == "#17181a",
            do:
              ~s(    <rect x="#{elem(pos, 0) + 1.5}" y="#{elem(pos, 1) + 1.5}" width="55" height="55" rx="7.5" fill="none" stroke="#f2c14e" stroke-width="1.5"/>\n),
            else: ""

        "    #{rect(pos, fill)}\n" <>
          rim <>
          ~s(    <path d="#{digits(pos, ds)}" fill="none" stroke="#{ink}" stroke-width="4.5" stroke-linecap="round"/>\n)
      end)

    head("00 61 73 6D: the four bytes every WebAssembly module starts with, \\0asm.") <>
      "  <g>\n" <> tiles <> "  </g>\n</svg>\n"
  end

  def small do
    tiles = Enum.map_join(@tiles, "", fn {pos, _, fill, _} -> "  #{rect(pos, fill)}\n" end)
    head("The 16px cut of logo.svg: the four tiles of 00 61 73 6D without their digits.") <> tiles <> "</svg>\n"
  end

  # One ink: tiles printed in currentColor, the digits knocked out of them.
  def stamp do
    cuts = Enum.map_join(@tiles, " ", fn {pos, ds, _, _} -> digits(pos, ds) end)
    tiles = Enum.map_join(@tiles, "", fn {pos, _, _, _} -> "          #{rect(pos, "currentColor")}\n" end)

    """
          <symbol id="mark" viewBox="0 0 128 128">
            <mask id="mark-cut">
              <rect width="128" height="128" fill="#fff"/>
              <path d="#{cuts}" fill="none" stroke="#000" stroke-width="4.5" stroke-linecap="round"/>
            </mask>
            <g mask="url(#mark-cut)">
    #{tiles}        </g>
          </symbol>
    """
  end
end

root = Path.expand("..", __DIR__)
File.write!(Path.join(root, "assets/logo.svg"), Logo.full())
File.write!(Path.join(root, "site/favicon.svg"), Logo.full())
File.write!(Path.join(root, "assets/logo-small.svg"), Logo.small())
IO.write(Logo.stamp())
