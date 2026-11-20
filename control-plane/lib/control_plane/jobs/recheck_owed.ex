defmodule ControlPlane.Jobs.RecheckOwed do
  @moduledoc "Judge again the tickets whose last completeness verdict was owed, once what they wait for may have come due."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 3000]

  @impl Oban.Worker
  def perform(_job) do
    ControlPlane.Evaluations.recheck()
    :ok
  end
end
