defmodule ControlPlane.Repo.Migrations.AddRerunsToWorkloads do
  use Ecto.Migration

  def change do
    alter table(:workloads) do
      # How many times each check on the workload's pull request was re-run, by name.
      add :reruns, :map, null: false, default: %{}
    end
  end
end
