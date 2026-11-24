defmodule ControlPlane.Jobs.SettleWorkload do
  @moduledoc """
  Settle one workload now. A workload ending and a review finishing are caused by the control
  plane, so no webhook announces them and waiting for the backstop would be an hour's delay.
  """

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [keys: [:workload_id], period: 30]

  alias ControlPlane.{PullRequests, Settle, Workloads}

  @impl Oban.Worker
  def perform(%Oban.Job{args: %{"workload_id" => id}}) do
    workload = Workloads.get(id)
    Settle.one(workload)
    record_pull_request(workload)
    :ok
  end

  # A workload that ends with a pull request is what makes the control plane know of it, and no
  # webhook announces that: the record is made now rather than at the next poll.
  defp record_pull_request(%ControlPlane.Workload{} = workload) do
    with {:ok, number} <- Workloads.pull_request_number(workload) do
      PullRequests.refresh(workload.repository, number)
    end
  end

  defp record_pull_request(_), do: :ok
end
