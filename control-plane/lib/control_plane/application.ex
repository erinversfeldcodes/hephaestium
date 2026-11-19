defmodule ControlPlane.Application do
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    # What was in flight before this moment belonged to a control plane that stopped.
    started_at = DateTime.utc_now()

    children = [
      ControlPlane.Repo,
      {Registry, keys: :unique, name: ControlPlane.RunnerRegistry},
      {Oban, Application.fetch_env!(:control_plane, Oban)}
    ]

    with {:ok, supervisor} <-
           Supervisor.start_link(children, strategy: :one_for_one, name: ControlPlane.Supervisor) do
      if recover_on_start?(Application.get_env(:control_plane, :recover_on_start)) do
        Task.start(fn -> ControlPlane.Recovery.run(started_at) end)
      end

      {:ok, supervisor}
    end
  end

  @doc """
  Whether the workloads a stopped control plane left running are failed at start. On unless
  the setting says otherwise: the test suite turns it off, so tests do not fail each other's
  rows.
  """
  @spec recover_on_start?(boolean() | nil) :: boolean()
  def recover_on_start?(false), do: false
  def recover_on_start?(_setting), do: true
end
