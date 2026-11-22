defmodule ControlPlane.SpendTaskTest do
  @moduledoc """
  `mix spend` prints the roll-ups of the ledger for a person to read, so spend can be read
  without SQL.
  """

  use ControlPlane.DataCase

  import ControlPlane.SpendFixtures

  alias Mix.Tasks.Spend, as: Task

  @repo "erinversfeldcodes/gazebolang"

  setup do
    Mix.shell(Mix.Shell.Process)
    previous = Application.get_env(:control_plane, :intake)

    on_exit(fn ->
      Mix.shell(Mix.Shell.IO)
      Application.put_env(:control_plane, :intake, previous)
    end)

    :ok
  end

  defp printed do
    receive do
      {:mix_shell, :info, [text]} -> text
    after
      100 -> flunk("nothing was printed")
    end
  end

  test "prints a ticket's roll-up for the repository it is given" do
    spent(12, [usd(0.5)])

    Task.run(["--repo", @repo, "--ticket", "12"])

    assert printed() =~ "Ticket #12 in #{@repo}: $0.500 across 1 workload"
  end

  test "takes the repository from the configuration when exactly one is configured" do
    Application.put_env(:control_plane, :intake, repositories: [@repo])
    spent(12, [usd(0.5)], epic: 7)

    Task.run(["--epic", "7"])

    assert printed() =~ "Epic #7 in #{@repo}"
  end

  test "prints a branch and the last days" do
    spent(12, [usd(0.5)], branch: "heph/12-a-fix", at: DateTime.to_iso8601(DateTime.utc_now()))

    Task.run(["--repo", @repo, "--branch", "heph/12-a-fix"])
    assert printed() =~ "Branch heph/12-a-fix"

    Task.run(["--repo", @repo, "--days", "3"])
    assert printed() =~ "Spent in #{@repo} over the last 3 days"
  end

  test "asks for a repository when it cannot tell which, and says how" do
    Application.put_env(:control_plane, :intake, repositories: [@repo, "someone/else"])

    error = assert_raise Mix.Error, fn -> Task.run(["--ticket", "12"]) end

    assert error.message =~ "--repo"
  end

  test "wants exactly one of a ticket, an epic, a branch or days, and says so" do
    for args <- [["--repo", @repo], ["--repo", @repo, "--ticket", "1", "--epic", "2"]] do
      error = assert_raise Mix.Error, fn -> Task.run(args) end
      assert error.message =~ "--ticket"
    end
  end
end
