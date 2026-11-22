defmodule ControlPlane.Jobs.RollUpEpics do
  @moduledoc "Ask for a judgement of each epic whose sub-issues have all closed."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 300]

  @impl Oban.Worker
  def perform(_job) do
    results = ControlPlane.Evaluations.epics()
    ControlPlane.Webhooks.missed(:ticket_closed, Enum.count(results, &match?({:started, _}, &1)))
    :ok
  end
end
