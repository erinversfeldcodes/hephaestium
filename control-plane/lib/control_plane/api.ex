defmodule ControlPlane.Api do
  @moduledoc """
  The control plane's public HTTP surface: a login with GitHub and read-only endpoints for what
  a dashboard shows, behind the session the login makes. It is a listener of its own, started
  only when a port is set, and it refuses to start without the key that signs its sessions.
  """

  alias ControlPlane.Api.{Router, Settings, UsedStates}

  @doc """
  The children the application supervises for the API: none when it is not configured (no
  port), otherwise the store of used login states and the listener. Settings that cannot run
  the API raise here, so the control plane does not start half open.
  """
  @spec children(keyword() | nil) :: [Supervisor.child_spec() | module()]
  def children(nil), do: []

  def children(config) do
    settings = Settings.build!(config)

    [
      UsedStates,
      {Bandit,
       plug: {Router, settings: settings}, scheme: :http, port: Keyword.fetch!(config, :port)}
    ]
  end
end
