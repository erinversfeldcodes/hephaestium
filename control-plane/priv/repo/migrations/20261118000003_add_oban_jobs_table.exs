defmodule ControlPlane.Repo.Migrations.AddObanJobsTable do
  use Ecto.Migration

  def up, do: Oban.Migration.up(version: 14)

  # An older version is the one to go back to; 1 removes the table.
  def down, do: Oban.Migration.down(version: 1)
end
