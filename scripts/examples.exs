#!/usr/bin/env elixir

# Run every example's end-to-end suite (examples/*/test/e2e) against an ampoule binary: each
# suite packs its app with `ampoule release`, runs the binary and checks it from outside.
#
#   elixir scripts/examples.exs [NAME...] [--ampoule PATH]
#
#   NAME              only these examples (default: all of them)
#   --ampoule PATH  the binary to test (default: runner/target/release/ampoule)
#
# phoenix_ecto needs Postgres: $E2E_DATABASE_URL, a server URL without a database name
# (default ecto://postgres:postgres@127.0.0.1:5432). rustler needs cargo with the
# wasm32-wasip1-threads target. NIF builds use AMPOULE_WASI_SDK / AMPOULE_KIT (default:
# build/wasi-sdk and build/engine/kit).

defmodule Examples do
  @root Path.expand("..", __DIR__)
  @shared Path.join(@root, "examples/_shared/packed_app.ex")

  def run(argv) do
    {opts, names} = OptionParser.parse!(argv, strict: [ampoule: :string])
    ampoule = Path.expand(Keyword.get(opts, :ampoule, Path.join(@root, "runner/target/release/ampoule")))
    File.exists?(ampoule) || fail("#{ampoule} not found: build it with `cargo build --release` in runner/")

    all =
      Path.join(@root, "examples/*/mix.exs")
      |> Path.wildcard()
      |> Enum.map(&(&1 |> Path.dirname() |> Path.basename()))
      |> Enum.sort()

    names = if names == [], do: all, else: names
    for n <- names, n not in all, do: fail("no example #{n} (examples: #{Enum.join(all, ", ")})")

    env = [
      {"AMPOULE", ampoule},
      {"AMPOULE_WASI_SDK", System.get_env("AMPOULE_WASI_SDK", Path.join(@root, "build/wasi-sdk"))},
      {"AMPOULE_KIT", System.get_env("AMPOULE_KIT", Path.join(@root, "build/engine/kit"))},
      {"MIX_ENV", "test"}
    ]

    failed =
      for name <- names, not passes?(name, env) do
        name
      end

    if failed == [] do
      IO.puts(IO.ANSI.green() <> "examples: all passed (#{Enum.join(names, ", ")})" <> IO.ANSI.reset())
    else
      fail("failed: #{Enum.join(failed, ", ")}")
    end
  end

  defp passes?(name, env) do
    dir = Path.join(@root, "examples/#{name}")
    IO.puts(IO.ANSI.bright() <> "==> #{name}" <> IO.ANSI.reset())

    # Each example carries its own copy so it can be lifted out on its own; keep them identical.
    if File.read!(Path.join(dir, "test/support/packed_app.ex")) != File.read!(@shared) do
      IO.puts(:stderr, "#{name}/test/support/packed_app.ex differs from examples/_shared/packed_app.ex")
      false
    else
      mix(["deps.get"], dir, env) and mix(["test"], dir, env)
    end
  end

  defp mix(args, dir, env) do
    {_, status} = System.cmd("mix", args, cd: dir, env: env, into: IO.stream(), stderr_to_stdout: true)
    status == 0
  end

  defp fail(msg) do
    IO.puts(:stderr, IO.ANSI.red() <> "examples: " <> msg <> IO.ANSI.reset())
    System.halt(1)
  end
end

Examples.run(System.argv())
