defmodule ControlPlane.SettleAnnouncementTest do
  # Not in the shared sandbox: every caller below holds a connection of its own, so the claim is
  # shown to hold between separate transactions and not by the one connection a sandbox shares.
  use ExUnit.Case, async: false

  alias ControlPlane.{Repo, SettleAnnouncement}

  @repository "erinversfeldcodes/race-#{System.unique_integer([:positive])}"

  setup do
    Ecto.Adapters.SQL.Sandbox.mode(Repo, :auto)

    on_exit(fn ->
      Repo.query!("delete from settle_announcements where repository = $1", [@repository])
      Ecto.Adapters.SQL.Sandbox.mode(Repo, :manual)
    end)
  end

  test "of twenty callers at once for one head and state, exactly one holds the claim" do
    claimed =
      1..20
      |> Task.async_stream(fn _ -> SettleAnnouncement.claim(@repository, 7, "abc", "done") end,
        max_concurrency: 20
      )
      |> Enum.map(fn {:ok, won} -> won end)

    assert Enum.count(claimed, & &1) == 1
  end

  test "another head or another state is its own claim, and a released claim can be made again" do
    assert SettleAnnouncement.claim(@repository, 8, "abc", "done")
    refute SettleAnnouncement.claim(@repository, 8, "abc", "done")
    assert SettleAnnouncement.claim(@repository, 8, "def", "done")
    assert SettleAnnouncement.claim(@repository, 8, "abc", "failed")

    SettleAnnouncement.release(@repository, 8, "abc", "done")
    assert SettleAnnouncement.claim(@repository, 8, "abc", "done")
  end
end
