defmodule ControlPlane.Repo.Migrations.CreateSettleAnnouncements do
  use Ecto.Migration

  # One row for each announcement a settle pass made about a pull request: its head commit and
  # the state it announced. The row is claimed before the comment is written, so passes and
  # deliveries that run at once, or for the several workloads a ticket has had, find the claim
  # and say nothing.
  def change do
    create table(:settle_announcements, primary_key: false) do
      add :repository, :text, primary_key: true
      add :number, :bigint, primary_key: true
      add :head_sha, :text, primary_key: true
      add :state, :text, primary_key: true
      add :announced_at, :utc_datetime_usec, null: false
    end
  end
end
