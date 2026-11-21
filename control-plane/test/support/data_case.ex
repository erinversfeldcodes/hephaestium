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

    # What a test sets for the settle and intake configuration is its own: whichever test ran
    # before must not decide what the next one sees, as a random order would otherwise let it.
    saved = for key <- [:settle, :intake], do: {key, Application.fetch_env(:control_plane, key)}

    on_exit(fn ->
      for {key, saved} <- saved do
        case saved do
          {:ok, value} -> Application.put_env(:control_plane, key, value)
          :error -> Application.delete_env(:control_plane, key)
        end
      end

      Ecto.Adapters.SQL.Sandbox.stop_owner(pid)
    end)

    :ok
  end
end
