defmodule ControlPlane.ExecutorLink do
  @moduledoc """
  Whether executors on developers' machines run the workloads, and how that is set.

  Off, the control plane runs each workload with an executor it starts itself, as it always
  has, and no route of the link exists. On, a workload is left queued for the executor of the
  person who approved its ticket to claim over the API, which is the one way a workload runs.
  The two are alternatives chosen by configuration, never both at once: a workload that a
  local job and a remote executor could both take would run twice.
  """

  @defaults [
    # The silence the local runner tolerates from an executor before it stops it.
    lease_seconds: 120,
    # How long a claim waits for a workload before it answers that there is none. Kept under
    # the idle time a proxy in front of the control plane allows a request, so the proxy never
    # cuts a claim that is only waiting.
    claim_wait_ms: 25_000,
    # How often leases that have lapsed are looked for.
    reap_ms: 5_000,
    # A workload that is lost is put back on the queue this many times at most, so a ticket
    # that crashes every executor does not do so for ever.
    max_requeues: 2
  ]

  @doc "Whether the executor link is on."
  @spec enabled?() :: boolean()
  def enabled?, do: config(:enabled, false) == true

  @doc "A setting of the link, or its default."
  @spec config(atom(), term()) :: term()
  def config(key, default \\ nil) do
    settings = Application.get_env(:control_plane, :executor_link, [])
    Keyword.get(settings, key, Keyword.get(@defaults, key, default))
  end

  @doc """
  How long after a claim that did not find a workload an executor still counts as connected:
  a claim that waits and a pause between claims, with room for a slow answer.
  """
  @spec connected_for_ms() :: pos_integer()
  def connected_for_ms, do: config(:claim_wait_ms) + 30_000
end
