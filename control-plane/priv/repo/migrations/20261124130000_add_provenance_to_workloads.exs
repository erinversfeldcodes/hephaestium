defmodule ControlPlane.Repo.Migrations.AddProvenanceToWorkloads do
  use Ecto.Migration

  def change do
    alter table(:workloads) do
      # What the control plane ran under when it made the workload: its own build and, for a
      # review or an evaluation, the Athenaeum release it read. Null for a workload kept
      # before this was recorded, which is unknown and never filled in from today's setting.
      add :provenance, :map
    end
  end
end
