defmodule ControlPlane.Repo.Migrations.CreateWebhookDeliveries do
  use Ecto.Migration

  # One row for each delivery a code host sent, by the id it gave it. The unique index is what
  # makes a redelivery one job: it holds for as long as the row does, which is longer than
  # Oban keeps a job.
  def change do
    create table(:webhook_deliveries) do
      add :source, :text, null: false
      add :delivery_id, :text, null: false
      add :received_at, :utc_datetime_usec, null: false
    end

    create unique_index(:webhook_deliveries, [:source, :delivery_id])
    create index(:webhook_deliveries, [:received_at])
  end
end
