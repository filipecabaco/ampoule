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

  test "FNV-1a matches the published test vector", %{bin: bin} do
    {out, 0} = PackedApp.run(bin, ["eval", ~S|IO.puts(Checksum.fnv1a("a"))|])
    # FNV-1a 64 of "a" is 0xaf63dc4c8601ec8c.
    assert out =~ Integer.to_string(0xAF63DC4C8601EC8C)
  end

  test "a bad argument raises, as from any NIF", %{bin: bin} do
    {out, _} = PackedApp.run(bin, ["eval", "Checksum.crc32(:not_a_binary)"])
    assert out =~ "ArgumentError"
  end
end
