defmodule ControlPlane.Jobs.SweepWorkspaces do
  @moduledoc "Delete the checkouts that finished runs no longer need, once a failed run's window to be looked at has passed."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 3000]

  @impl Oban.Worker
  def perform(_job) do
    ControlPlane.Workspaces.sweep()
    :ok
  end
end
