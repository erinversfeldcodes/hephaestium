defmodule ControlPlane.Repo.Migrations.AddResumeToWorkloads do
  use Ecto.Migration

  def change do
    alter table(:workloads) do
      # The approval this workload was resumed for (#131): a maintainer's re-approval of a
      # ticket whose pull request is open. An approval resumes a pull request once.
      add :resumed_for, :utc_datetime_usec
      # The revision attempt the pull request had reached when it was resumed, so the
      # revisions a resume grants are counted from there, not from the first.
      add :budget_base, :integer, null: false, default: 0
    end
  end
end
