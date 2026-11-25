defmodule ControlPlane.Application do
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    # What was in flight before this moment belonged to a control plane that stopped.
    started_at = DateTime.utc_now()

    if ControlPlane.Webhooks.enabled?(), do: ControlPlane.Webhooks.check!()

    oban =
      ControlPlane.Schedule.oban(
        Application.fetch_env!(:control_plane, Oban),
        ControlPlane.Webhooks.enabled?()
      )

    children = [
      ControlPlane.Repo,
      {Registry, keys: :unique, name: ControlPlane.RunnerRegistry},
      {Oban, oban}
    ]

    children = children ++ health_listener(Application.get_env(:control_plane, :health))
    children = children ++ ControlPlane.Api.children(Application.get_env(:control_plane, :api))
    children = children ++ executor_link()

    with {:ok, supervisor} <-
           Supervisor.start_link(children, strategy: :one_for_one, name: ControlPlane.Supervisor) do
      if recover_on_start?(Application.get_env(:control_plane, :recover_on_start)) do
        Task.start(fn ->
          ControlPlane.Recovery.run(started_at)
          ControlPlane.Recovery.end_stale_jobs(started_at)
          ControlPlane.Ledger.backfill()
        end)

        # A restart is when deliveries are lost: look at everything once.
        ControlPlane.Webhooks.start_pass()
      end

      {:ok, supervisor}
    end
  end

  # The sweep for lapsed leases, only when executors on developers' machines run the work and
  # recurring jobs are not turned off (it polls the database on a timer).
  defp executor_link do
    if ControlPlane.ExecutorLink.enabled?() and ControlPlane.ExecutorLink.config(:reap, true),
      do: [ControlPlane.ExecutorLink.Reaper],
      else: []
  end

  # The listener the platform's health check calls. Not started unless a port is set, so a
  # control plane run by hand opens no port it did not open before.
  defp health_listener(nil), do: []

  defp health_listener(settings) do
    [
      {Bandit,
       plug: ControlPlane.Health.Plug, scheme: :http, port: Keyword.fetch!(settings, :port)}
    ]
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
