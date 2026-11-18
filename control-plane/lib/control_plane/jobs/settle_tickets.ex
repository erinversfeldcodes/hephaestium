defmodule ControlPlane.Jobs.SettleTickets do
  @moduledoc "Settle the tickets whose workloads completed, once their pull requests are green, red or closed."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 50]

  @impl Oban.Worker
  def perform(_job) do
    ControlPlane.Settle.run()
    :ok
  end
end
