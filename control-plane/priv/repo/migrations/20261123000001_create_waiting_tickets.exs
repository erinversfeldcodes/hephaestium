defmodule ControlPlane.Repo.Migrations.CreateWaitingTickets do
  use Ecto.Migration

  def change do
    # An approved ticket that has no workload yet, with the time its approval was given. The
    # row is removed when a workload is made, when the approval stops standing and when the
    # ticket closes, so the board can show waiting work without asking the tracker.
    create table(:waiting_tickets, primary_key: false) do
      add :repository, :text, primary_key: true
      add :ticket_number, :bigint, primary_key: true
      add :title, :text
      add :approved_by, :text, null: false
      add :approved_at, :utc_datetime, null: false

      timestamps(type: :utc_datetime)
    end
  end
end
