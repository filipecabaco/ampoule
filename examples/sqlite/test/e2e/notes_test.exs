defmodule Notes.E2ETest do
  # Packs the app, with exqlite's C NIF (SQLite) rebuilt for wasm, and runs the binary twice
  # against one database file in a host directory mounted at /data.
  use ExUnit.Case, async: false

  setup_all do
    %{bin: PackedApp.pack!()}
  end

  setup do
    dir = Path.join(System.tmp_dir!(), "notes-e2e-#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    on_exit(fn -> File.rm_rf!(dir) end)
    %{data: dir}
  end

  defp run(bin, data, code), do: PackedApp.run(bin, ["--mount", "#{data}:/data", "eval", code])

  test "notes outlive the process, in a SQLite file on the host", %{bin: bin, data: data} do
    {out, 0} = run(bin, data, ~S|IO.inspect(Notes.add("first")); IO.inspect(Notes.add("second"))|)
    assert out =~ "1\n2"

    {out, 0} = run(bin, data, "IO.inspect(Notes.all())")
    assert out =~ ~s([{1, "first"}, {2, "second"}])

    # A real SQLite database, which any SQLite tool on the host can open.
    assert <<"SQLite format 3", 0, _::binary>> = File.read!(Path.join(data, "notes.db"))
  end

  test "the binary carries its own SQLite", %{bin: bin, data: data} do
    {out, 0} = run(bin, data, "IO.puts(Notes.sqlite_version())")
    assert out =~ ~r/^3\.\d+\.\d+/m
  end
end
