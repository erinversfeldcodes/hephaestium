defmodule ControlPlane.ExecutorLink.Reaper do
  @moduledoc """
  Looks for leases that have lapsed on a timer, so a workload whose executor is gone is put
  back or failed even when no executor is asking for work. A claim and each request also look,
  and a lapse is refused whether or not this has run yet, so this decides when a lost workload
  is dealt with and never whether a lapsed executor may go on.
  """

  use GenServer

  require Logger

  alias ControlPlane.{ExecutorLink, Leases}

  def start_link(opts), do: GenServer.start_link(__MODULE__, opts, name: __MODULE__)

  @impl true
  def init(_opts) do
    schedule()
    {:ok, nil}
  end

  @impl true
  def handle_info(:reap, state) do
    try do
      Leases.reap()
    rescue
      error -> Logger.error("could not look for lapsed leases: #{Exception.message(error)}")
    end

    schedule()
    {:noreply, state}
  end

  defp schedule, do: Process.send_after(self(), :reap, ExecutorLink.config(:reap_ms))
end
