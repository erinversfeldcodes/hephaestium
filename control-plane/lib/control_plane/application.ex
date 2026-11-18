defmodule ControlPlane.Application do
  @moduledoc false

  use Application

  @impl true
  def start(_type, _args) do
    children = [ControlPlane.Repo]
    Supervisor.start_link(children, strategy: :one_for_one, name: ControlPlane.Supervisor)
  end
end
