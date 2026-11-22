defmodule ControlPlane.GitHubParentTest do
  @moduledoc """
  The tracker's parent of a ticket, read from GitHub as the shape of its parent-issue answer
  recorded in the stand-in. That GitHub answers this way is from its documentation and is not
  proved here.
  """

  use ExUnit.Case, async: false

  alias ControlPlane.{StandInGitHub, Ticket, Tracker}

  @repo "erinversfeldcodes/gazebolang"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-parent-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)
    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [@repo])

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      File.rm_rf(dir)
    end)

    %{gh: gh}
  end

  test "is the issue GitHub names as the parent, with its progress", %{gh: gh} do
    StandInGitHub.respond(gh, "repos/#{@repo}/issues/12/parent", %{
      "number" => 5,
      "state" => "open",
      "labels" => [],
      "sub_issues_summary" => %{"total" => 2, "completed" => 2}
    })

    assert {:ok, %Ticket{key: 5, progress: %{total: 2, completed: 2}}} = Tracker.parent(@repo, 12)
  end

  test "is nothing for a ticket that has no parent", %{gh: gh} do
    StandInGitHub.fail(
      gh,
      "repos/#{@repo}/issues/12/parent",
      ~s({"message":"Not Found","status":"404"})
    )

    assert Tracker.parent(@repo, 12) == {:ok, nil}
  end

  test "is an error for any other failure", %{gh: gh} do
    StandInGitHub.fail(
      gh,
      "repos/#{@repo}/issues/12/parent",
      ~s({"message":"Server Error","status":"500"})
    )

    assert {:error, _} = Tracker.parent(@repo, 12)
  end
end
