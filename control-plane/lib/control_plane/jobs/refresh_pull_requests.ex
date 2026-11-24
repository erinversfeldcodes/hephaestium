defmodule ControlPlane.Jobs.RefreshPullRequests do
  @moduledoc """
  Once an hour, make the record of pull requests right again: store the ones a workload opened
  that have no row, and read again each open one the code host has not confirmed for an hour,
  so that a webhook delivery that never came leaves the record wrong for no longer than that.
  """

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 300]

  require Logger

  @impl Oban.Worker
  def perform(_job) do
    results = ControlPlane.PullRequests.refresh_due(DateTime.utc_now())

    for {:error, message} <- results,
        do: Logger.warning("could not refresh a pull request's record: #{message}")

    :ok
  end
end
