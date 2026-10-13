defmodule TextKit.E2ETest do
  # Each test evaluates Elixir inside the packed binary, where the NIF runs as wasm linked into
  # the engine, and checks what it prints.
  use ExUnit.Case, async: true

  setup_all do
    %{bin: PackedApp.pack!()}
  end

  defp eval!(bin, expr) do
    {out, status} = PackedApp.run(bin, ["eval", expr])
    assert status == 0, out
    out |> String.trim() |> String.split("\n") |> List.last()
  end

  test "the NIF is linked into the packed engine", %{bin: bin} do
    {out, 0} = PackedApp.run(bin, ["info"])
    assert out =~ ~r/nifs:\s+textkit/
  end

  test "slugify", %{bin: bin} do
    assert eval!(bin, ~s|IO.puts(TextKit.slugify("Héllo, World -- from WASM!"))|) == "héllo-world-from-wasm"
  end

  test "word_count returns a map", %{bin: bin} do
    assert eval!(bin, ~s|TextKit.word_count("the cat and THE hat") \|> Enum.sort() \|> inspect() \|> IO.puts()|) ==
             ~s|[{"and", 1}, {"cat", 1}, {"hat", 1}, {"the", 2}]|
  end

  test "sha256_hex on a dirty scheduler matches :crypto", %{bin: bin} do
    expr =
      ~s|data = :binary.copy("ampoule", 1_000_000); IO.puts(TextKit.sha256_hex(data) == Base.encode16(:crypto.hash(:sha256, data), case: :lower))|

    assert eval!(bin, expr) == "true"
  end

  test "Rust results become ok/error tuples", %{bin: bin} do
    assert eval!(bin, ~s|IO.inspect({TextKit.parse_int(" 42 "), TextKit.parse_int("forty-two")})|) ==
             "{{:ok, 42}, {:error, :invalid}}"
  end

  test "is safe to call from many processes at once", %{bin: bin} do
    expr =
      ~s|1..2000 \|> Task.async_stream(&TextKit.slugify("item #\{&1\}")) \|> Enum.count(&match?({:ok, "item-" <> _}, &1)) \|> IO.puts()|

    assert eval!(bin, expr) == "2000"
  end

  test "custom commands from ampoule.toml", %{bin: bin} do
    {out, 0} = PackedApp.run(bin, ["demo"])
    assert out =~ "hello-from-rust-inside-wasm"
  end
end
