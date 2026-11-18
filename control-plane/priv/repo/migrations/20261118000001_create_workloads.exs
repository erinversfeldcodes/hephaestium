defmodule ControlPlane.Repo.Migrations.CreateWorkloads do
  use Ecto.Migration

  def change do
    create table(:workloads, primary_key: false) do
      add :id, :uuid, primary_key: true
      add :repository, :text, null: false
      add :ticket_number, :bigint, null: false
      add :state, :text, null: false, default: "queued"
      # The workload as it was given, in the contract's JSON mapping.
      add :workload, :map, null: false
      # What the executor kept of the run, when it has ended.
      add :record, :map
      add :reason, :text
      add :started_at, :utc_datetime_usec
      add :finished_at, :utc_datetime_usec

      timestamps()
    end

    create index(:workloads, [:repository, :ticket_number])
    create index(:workloads, [:state])

    create constraint(:workloads, :state_is_known,
             check:
               "state in ('queued', 'preparing', 'running', 'gating', 'completed', 'failed', 'blocked', 'timed_out', 'cancelled')"
           )
  end
end
