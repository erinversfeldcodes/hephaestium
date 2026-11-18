defmodule ControlPlane.DataCase do
  @moduledoc """
  A test that uses the database, in a transaction of its own that is rolled back.
  """

  use ExUnit.CaseTemplate

  using do
    quote do
      alias ControlPlane.Repo
    end
  end

  setup tags do
    pid = Ecto.Adapters.SQL.Sandbox.start_owner!(ControlPlane.Repo, shared: not tags[:async])
    on_exit(fn -> Ecto.Adapters.SQL.Sandbox.stop_owner(pid) end)
    :ok
  end
end
