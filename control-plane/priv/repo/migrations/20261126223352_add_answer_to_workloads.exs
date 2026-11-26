defmodule ControlPlane.Repo.Migrations.AddAnswerToWorkloads do
  use Ecto.Migration

  def change do
    alter table(:workloads) do
      # What a person answered to the question this workload ended blocked with, kept so that
      # every later workload on the ticket is given it. Null: no question was answered here.
      add :answer, :text
    end
  end
end
