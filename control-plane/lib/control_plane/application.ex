defmodule ControlPlane.Application do
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [
      ControlPlane.Repo,
      {Registry, keys: :unique, name: ControlPlane.RunnerRegistry},
      {Oban, Application.fetch_env!(:control_plane, Oban)}
    ]

    Supervisor.start_link(children, strategy: :one_for_one, name: ControlPlane.Supervisor)
  end
end
