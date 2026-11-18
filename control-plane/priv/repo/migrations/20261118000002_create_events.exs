defmodule ControlPlane.Repo.Migrations.CreateEvents do
  use Ecto.Migration

  def up do
    create table(:events, primary_key: false) do
      add :id, :bigserial, primary_key: true
      add :workload_id, references(:workloads, type: :uuid, on_delete: :restrict), null: false
      add :sequence, :bigint, null: false
      add :time, :utc_datetime_usec, null: false
      # The kind of the event, lifted out of it to be queried.
      add :kind, :text, null: false
      # The event as the contract wrote it, whole.
      add :event, :map, null: false

      timestamps(updated_at: false)
    end

    # Two writers of one sequence cannot both succeed.
    create unique_index(:events, [:workload_id, :sequence])
    create constraint(:events, :sequence_starts_at_one, check: "sequence >= 1")

    # The record is append-only: nothing changes or removes an event, whatever
    # asks, so that what was said to have happened stays what was said.
    execute """
    CREATE FUNCTION events_are_append_only() RETURNS trigger AS $$
    BEGIN
      RAISE EXCEPTION 'events are append-only: an event cannot be changed or deleted';
    END;
    $$ LANGUAGE plpgsql
    """

    execute """
    CREATE TRIGGER events_append_only
    BEFORE UPDATE OR DELETE ON events
    FOR EACH ROW EXECUTE FUNCTION events_are_append_only()
    """
  end

  def down do
    execute "DROP TRIGGER events_append_only ON events"
    execute "DROP FUNCTION events_are_append_only()"
    drop table(:events)
  end
end
