defmodule ControlPlane.Jobs.SettleTickets do
  @moduledoc "Settle the tickets whose workloads completed, once their pull requests are green, red or closed."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 50]

  @impl Oban.Worker
  def perform(_job) do
    results = ControlPlane.Settle.run()
    ControlPlane.Webhooks.missed(:checks_completed, Enum.count(results, &acted?/1))
    :ok
  end

  # A workload still waiting for its checks is not work a delivery missed.
  defp acted?(%{result: {:waiting, _}}), do: false
  defp acted?(_), do: true
end
