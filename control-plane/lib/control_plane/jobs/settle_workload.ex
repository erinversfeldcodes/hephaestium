defmodule ControlPlane.Jobs.SettleWorkload do
  @moduledoc """
  Settle one workload now. A workload ending and a review finishing are caused by the control
  plane, so no webhook announces them and waiting for the backstop would be an hour's delay.
  """

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [keys: [:workload_id], period: 30]

  alias ControlPlane.{Settle, Workloads}

  @impl Oban.Worker
  def perform(%Oban.Job{args: %{"workload_id" => id}}) do
    Settle.one(Workloads.get(id))
    :ok
  end
end
