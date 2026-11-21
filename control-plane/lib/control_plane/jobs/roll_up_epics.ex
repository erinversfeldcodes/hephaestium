defmodule ControlPlane.Jobs.RollUpEpics do
  @moduledoc "Ask for a judgement of each epic whose sub-issues have all closed."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 300]

  @impl Oban.Worker
  def perform(_job) do
    ControlPlane.Evaluations.epics()
    :ok
  end
end
