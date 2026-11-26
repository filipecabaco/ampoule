defmodule TextKit.RepackTest do
  use ExUnit.Case, async: false

  test "packing again compiles none of the NIF's dependencies" do
    PackedApp.pack!()
    ampoule = System.get_env("AMPOULE") || System.find_executable("ampoule")

    {out, 0} =
      System.cmd(ampoule, ["release"],
        env: [{"MIX_ENV", "prod"}, {"CARGO_TERM_COLOR", "never"}],
        stderr_to_stdout: true
      )

    assert out =~ "Finished `release` profile"
    refute out =~ "Compiling sha2"
    refute out =~ "Compiling rustler"
  end
end
