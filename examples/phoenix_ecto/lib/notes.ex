defmodule Notes do
  @moduledoc "Notes, stored in Postgres."

  import Ecto.Query
  alias Notes.{Note, Repo}

  def list_notes, do: Repo.all(from n in Note, order_by: [asc: n.id])

  def get_note(id), do: Repo.get(Note, id)

  def create_note(attrs), do: %Note{} |> Note.changeset(attrs) |> Repo.insert()

  def delete_note(%Note{} = note), do: Repo.delete(note)
end
