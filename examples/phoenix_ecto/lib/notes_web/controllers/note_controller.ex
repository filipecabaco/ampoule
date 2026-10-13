defmodule NotesWeb.NoteController do
  use Phoenix.Controller, formats: [:json]
  import Plug.Conn

  def index(conn, _params), do: json(conn, %{data: Enum.map(Notes.list_notes(), &render_note/1)})

  def create(conn, params) do
    case Notes.create_note(params) do
      {:ok, note} ->
        conn |> put_status(:created) |> json(%{data: render_note(note)})

      {:error, changeset} ->
        conn |> put_status(:unprocessable_entity) |> json(%{errors: errors(changeset)})
    end
  end

  def show(conn, %{"id" => id}) do
    case find(id) do
      nil -> not_found(conn)
      note -> json(conn, %{data: render_note(note)})
    end
  end

  def delete(conn, %{"id" => id}) do
    case find(id) do
      nil ->
        not_found(conn)

      note ->
        {:ok, _} = Notes.delete_note(note)
        send_resp(conn, :no_content, "")
    end
  end

  defp find(id) do
    case Integer.parse(id) do
      {id, ""} -> Notes.get_note(id)
      _ -> nil
    end
  end

  defp not_found(conn), do: conn |> put_status(:not_found) |> json(%{errors: %{detail: "Not Found"}})

  defp render_note(note), do: Map.take(note, [:id, :title, :body, :inserted_at])

  defp errors(changeset) do
    Ecto.Changeset.traverse_errors(changeset, fn {msg, opts} ->
      Regex.replace(~r"%{(\w+)}", msg, fn _, key ->
        opts |> Keyword.get(String.to_existing_atom(key), key) |> to_string()
      end)
    end)
  end
end
