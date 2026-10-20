defmodule Checksum.E2ETest do
  # Packs the app, whose own C NIF ampoule rebuilds for wasm, and checks the binary's answers
  # against Erlang's own CRC-32 and a known FNV-1a vector.
  use ExUnit.Case, async: false

  setup_all do
    %{bin: PackedApp.pack!()}
  end

  test "the C NIF agrees with :erlang.crc32/1, on the normal and the dirty scheduler", %{bin: bin} do
    code = """
    small = "The quick brown fox jumps over the lazy dog"
    big = :binary.copy("0123456789abcdef", 100_000)
    IO.puts("small: \#{Checksum.crc32(small) == :erlang.crc32(small)}")
    IO.puts("big: \#{Checksum.crc32(big) == :erlang.crc32(big)}")
    IO.puts("vector: \#{Checksum.crc32(small)}")
    """

    {out, 0} = PackedApp.run(bin, ["eval", code])
    assert out =~ "small: true"
    assert out =~ "big: true"
    # The standard check value for this string.
    assert out =~ "vector: 1095738169"
  end

  test "FNV-1a matches the published test vector, and an Elixir FNV-1a on a dirty-scheduler input", %{bin: bin} do
    code = ~S"""
    IO.puts("vector: #{Checksum.fnv1a("a")}")
    import Bitwise
    big = :binary.copy("ampoule", 20_000)
    reference = for <<b <- big>>, reduce: 0xCBF29CE484222325, do: (h -> ((h ^^^ b) * 0x100000001B3) &&& 0xFFFFFFFFFFFFFFFF)
    IO.puts("big: #{Checksum.fnv1a(big) == reference}")
    """

    {out, 0} = PackedApp.run(bin, ["eval", code])
    # FNV-1a 64 of "a" is 0xaf63dc4c8601ec8c.
    assert out =~ "vector: #{0xAF63DC4C8601EC8C}"
    assert out =~ "big: true"
  end

  test "a bad argument raises, as from any NIF", %{bin: bin} do
    {out, _} = PackedApp.run(bin, ["eval", "Checksum.crc32(:not_a_binary)"])
    assert out =~ "ArgumentError"
  end
end
