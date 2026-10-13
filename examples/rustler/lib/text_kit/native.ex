defmodule TextKit.Native do
  @moduledoc false
  use Rustler, otp_app: :textkit, crate: "textkit"

  def slugify(_text), do: :erlang.nif_error(:nif_not_loaded)
  def word_count(_text), do: :erlang.nif_error(:nif_not_loaded)
  def sha256_hex(_data), do: :erlang.nif_error(:nif_not_loaded)
  def parse_int(_text), do: :erlang.nif_error(:nif_not_loaded)
end
