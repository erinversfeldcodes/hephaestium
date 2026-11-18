defmodule ControlPlane.Repo.Migrations.AddParentToWorkloads do
  use Ecto.Migration

  def change do
    alter table(:workloads) do
      # The workload this one revises: the one whose pull request's checks failed.
      add :parent_id, references(:workloads, type: :binary_id, on_delete: :nothing)
    end

    create index(:workloads, [:parent_id])
  end
end
