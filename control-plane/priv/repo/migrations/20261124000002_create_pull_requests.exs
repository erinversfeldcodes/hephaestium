defmodule ControlPlane.Repo.Migrations.CreatePullRequests do
  use Ecto.Migration

  # One row for each pull request the control plane knows of, kept so that the board and the
  # ticket page can link it and show its checks without asking the code host. `changed_at` is
  # when its state or checks last differed; `refreshed_at` is when the code host last confirmed
  # them, which is what the hourly poll goes by to repair a delivery that never came.
  def change do
    create table(:pull_requests, primary_key: false) do
      add :repository, :text, primary_key: true
      add :number, :bigint, primary_key: true
      add :url, :text, null: false
      add :state, :text, null: false
      add :checks, :text, null: false
      add :head_sha, :text
      add :changed_at, :utc_datetime_usec, null: false
      add :refreshed_at, :utc_datetime_usec, null: false
    end

    create constraint(:pull_requests, :state_is_known,
             check: "state in ('open', 'merged', 'closed')"
           )

    create constraint(:pull_requests, :checks_is_known,
             check: "checks in ('passing', 'failing', 'pending', 'none')"
           )

    create index(:pull_requests, [:repository, :head_sha])
    create index(:pull_requests, [:state, :refreshed_at])
  end
end
