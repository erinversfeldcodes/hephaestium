defmodule ControlPlane.Jobs.EvaluateMerged do
  @moduledoc "Ask for a judgement of each ticket whose pull request has merged since the ticket was settled."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 300]

  @impl Oban.Worker
  def perform(_job) do
    ControlPlane.Evaluations.merged()
    :ok
  end
end
