defmodule Checksum do
  @moduledoc """
  CRC-32 and FNV-1a over binaries, in a C NIF (`c_src/checksum_nif.c`).
  """
  @on_load :load_nif

  @doc false
  def load_nif do
    :code.priv_dir(:checksum) |> Path.join("checksum_nif") |> String.to_charlist() |> :erlang.load_nif(0)
  end

  @doc "CRC-32 (IEEE), as `:erlang.crc32/1` computes it."
  def crc32(_binary), do: :erlang.nif_error(:not_loaded)

  @doc "64-bit FNV-1a."
  def fnv1a(_binary), do: :erlang.nif_error(:not_loaded)
end
