defmodule ControlPlane.ScratchRepo do
  @moduledoc """
  A second Repo, for tests that need a database that is not the suite's own: one that is
  created empty and migrated by the test, or one that cannot be reached.
  """

  use Ecto.Repo,
    otp_app: :control_plane,
    adapter: Ecto.Adapters.Postgres
end
