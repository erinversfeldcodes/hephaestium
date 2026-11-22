defmodule ControlPlane.Repo.Migrations.CreateSpendEntries do
  use Ecto.Migration

  # The ledger of what workloads spent (ADR 0036): one row for each cost-bearing step, written
  # once. It only grows. The database refuses an update or a delete, so that no code, now or
  # later, can rewrite what was spent.
  def up do
    create table(:spend_entries, primary_key: false) do
      add :id, :binary_id, primary_key: true
      # Not a foreign key: the ledger is the record of what was spent and is kept whether or not
      # the workload it came from is, and it must not stop a workload being removed.
      add :workload_id, :binary_id, null: false
      # The step's place in the run record's steps, or 0 for a workload that lists none.
      add :step, :integer, null: false
      add :kind, :string, null: false
      add :lens, :string
      add :round, :integer, null: false, default: 0
      # A unit is kept as it was said and never converted into another.
      add :unit, :string, null: false
      add :source, :string, null: false
      add :amount, :float, null: false
      # The parts of a token amount, when the runtime gave them.
      add :input, :float
      add :output, :float
      add :cache_creation, :float
      add :cache_read, :float
      add :repository, :string, null: false
      add :ticket_number, :integer, null: false
      # The ticket's parent when the workload was built.
      add :epic_number, :integer
      add :pull_request_number, :integer
      add :branch, :string
      add :occurred_at, :utc_datetime_usec, null: false
      add :recorded_at, :utc_datetime_usec, null: false
    end

    # A record kept twice counts once.
    create unique_index(:spend_entries, [:workload_id, :step, :unit, :source])
    create index(:spend_entries, [:repository, :ticket_number])
    create index(:spend_entries, [:repository, :epic_number])
    create index(:spend_entries, [:branch])
    create index(:spend_entries, [:occurred_at])

    execute """
            CREATE FUNCTION spend_entries_append_only() RETURNS trigger AS $$
            BEGIN
              RAISE EXCEPTION 'spend_entries is append-only: % is not allowed', TG_OP;
            END;
            $$ LANGUAGE plpgsql
            """,
            "DROP FUNCTION spend_entries_append_only()"

    execute """
            CREATE TRIGGER spend_entries_rows_append_only
            BEFORE UPDATE OR DELETE ON spend_entries
            FOR EACH ROW EXECUTE FUNCTION spend_entries_append_only()
            """,
            "DROP TRIGGER spend_entries_rows_append_only ON spend_entries"

    execute """
            CREATE TRIGGER spend_entries_table_append_only
            BEFORE TRUNCATE ON spend_entries
            FOR EACH STATEMENT EXECUTE FUNCTION spend_entries_append_only()
            """,
            "DROP TRIGGER spend_entries_table_append_only ON spend_entries"
  end

  def down do
    execute "DROP TRIGGER IF EXISTS spend_entries_rows_append_only ON spend_entries"
    execute "DROP TRIGGER IF EXISTS spend_entries_table_append_only ON spend_entries"
    execute "DROP FUNCTION IF EXISTS spend_entries_append_only()"
    drop table(:spend_entries)
  end
end
