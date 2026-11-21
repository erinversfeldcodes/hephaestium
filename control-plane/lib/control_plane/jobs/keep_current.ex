defmodule ControlPlane.Jobs.KeepCurrent do
  @moduledoc "Bring a pull request up to date with its base when it fell behind while it waited for a person."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 300]

  @impl Oban.Worker
  def perform(_job) do
    ControlPlane.Settle.stale()
    :ok
  end
end
