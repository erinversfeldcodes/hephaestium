defmodule ControlPlane.Repo.Migrations.AddSettledAtToWorkloads do
  use Ecto.Migration

  def change do
    alter table(:workloads) do
      # When the ticket was settled once its pull request was green, red or closed.
      add :settled_at, :utc_datetime_usec
    end
  end
end
