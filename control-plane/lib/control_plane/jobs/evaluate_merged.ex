defmodule ControlPlane.Jobs.EvaluateMerged do
  @moduledoc "Ask for a judgement of each ticket whose pull request has merged since the ticket was settled."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 300]

  @impl Oban.Worker
  def perform(_job) do
    results = ControlPlane.Evaluations.merged()
    ControlPlane.Webhooks.missed(:change_merged, Enum.count(results, &match?({:started, _}, &1)))
    :ok
  end
end
