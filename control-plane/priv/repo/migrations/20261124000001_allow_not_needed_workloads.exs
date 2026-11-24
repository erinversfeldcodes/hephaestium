defmodule ControlPlane.Repo.Migrations.AllowNotNeededWorkloads do
  use Ecto.Migration

  def up do
    drop constraint(:workloads, :state_is_known)

    create constraint(:workloads, :state_is_known,
             check:
               "state in ('queued', 'preparing', 'running', 'gating', 'completed', 'failed', 'blocked', 'timed_out', 'cancelled', 'not_needed')"
           )
  end

  def down do
    execute "update workloads set state = 'cancelled' where state = 'not_needed'"
    drop constraint(:workloads, :state_is_known)

    create constraint(:workloads, :state_is_known,
             check:
               "state in ('queued', 'preparing', 'running', 'gating', 'completed', 'failed', 'blocked', 'timed_out', 'cancelled')"
           )
  end
end
