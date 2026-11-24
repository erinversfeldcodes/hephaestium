defmodule ControlPlane.Repo.Migrations.AddReportUrlToWorkloads do
  use Ecto.Migration

  def change do
    alter table(:workloads) do
      # Where the comment that says what a review found was written on the pull request,
      # so a finding can be linked to the comment it was posted in.
      add :report_url, :string
    end
  end
end
