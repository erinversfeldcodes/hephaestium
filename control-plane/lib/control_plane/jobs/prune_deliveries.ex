defmodule ControlPlane.Jobs.PruneDeliveries do
  @moduledoc false

  use Oban.Worker, queue: :intake

  @impl Oban.Worker
  def perform(_job), do: raise("not built")
end
