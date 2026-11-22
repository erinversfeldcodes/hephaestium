defmodule ControlPlane.Jobs.PruneDeliveries do
  @moduledoc "Forget the ids of deliveries older than a week, which a host will not send again."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 3000]

  @impl Oban.Worker
  def perform(_job) do
    ControlPlane.Webhooks.prune(DateTime.utc_now())
    :ok
  end
end
