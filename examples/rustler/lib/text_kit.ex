defmodule TextKit do
  @moduledoc """
  Text utilities implemented in Rust (native/textkit), called through Rustler.
  """

  alias TextKit.Native

  @doc "Lowercase, with every run of non-alphanumeric characters turned into a single dash."
  defdelegate slugify(text), to: Native

  @doc "How often each word occurs (case-insensitive)."
  defdelegate word_count(text), to: Native

  @doc "The SHA-256 of `data`, hex-encoded. Runs on a dirty CPU scheduler."
  defdelegate sha256_hex(data), to: Native

  @doc "`{:ok, integer}`, or `{:error, :invalid}` when `text` is not an integer."
  defdelegate parse_int(text), to: Native
end
