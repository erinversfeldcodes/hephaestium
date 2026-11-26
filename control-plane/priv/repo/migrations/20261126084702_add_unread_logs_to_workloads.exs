defmodule ControlPlane.Repo.Migrations.AddUnreadLogsToWorkloads do
  use Ecto.Migration

  def change do
    alter table(:workloads) do
      # How many settle passes found the log of a failed check unreadable, by check run id.
      add :unread_logs, :map, null: false, default: %{}
    end
  end
end
