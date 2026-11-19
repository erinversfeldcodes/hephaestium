defmodule ControlPlane.Repo.Migrations.AddReportedAtToWorkloads do
  use Ecto.Migration

  def change do
    alter table(:workloads) do
      # When what a review found, or what a revision's agent said, was written on the
      # pull request. Claimed before it is written, so it is written once.
      add :reported_at, :utc_datetime_usec
    end
  end
end
