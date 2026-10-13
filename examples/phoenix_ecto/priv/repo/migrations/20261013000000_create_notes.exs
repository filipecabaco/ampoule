defmodule Notes.Repo.Migrations.CreateNotes do
  use Ecto.Migration

  def change do
    create table(:notes) do
      add :title, :string, null: false
      add :body, :text, null: false, default: ""
      timestamps(type: :utc_datetime)
    end
  end
end
