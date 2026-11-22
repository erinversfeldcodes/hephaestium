defmodule ControlPlane.Repo.Migrations.CreateWorkloadLeases do
  use Ecto.Migration

  def change do
    # One row for each workload an executor has held, for as long as the workload lives. The
    # row says who holds it now (when it is active) and how many times it has been put back on
    # the queue after its executor was lost.
    create table(:workload_leases, primary_key: false) do
      # Not a foreign key, as the ledger's rows are not: a lease says what happened to a
      # workload and must not stop one being removed or a table of workloads being truncated.
      add :workload_id, :uuid, primary_key: true

      add :token_id, references(:device_tokens, type: :uuid, on_delete: :nothing)
      add :active, :boolean, null: false, default: false
      add :expires_at, :utc_datetime_usec
      add :requeues, :integer, null: false, default: 0

      timestamps(type: :utc_datetime_usec)
    end

    create index(:workload_leases, [:active, :expires_at])
    create index(:workload_leases, [:token_id])
  end
end
