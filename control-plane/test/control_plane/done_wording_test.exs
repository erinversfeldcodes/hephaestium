defmodule ControlPlane.DoneWordingTest do
  @moduledoc """
  What the comment on a green, approved pull request claims (#126). Green checks and
  approving reviews do not say the ticket's goal is met, so the comment says what was
  looked at and leaves the goal to a person.
  """

  use ControlPlane.DataCase

  import ControlPlane.Fixtures

  alias ControlPlane.{StandInGitHub, Tickets, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @pr "https://github.com/erinversfeldcodes/gazebolang/pull/52"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-done-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)
    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [])

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      File.rm_rf(dir)
    end)

    {:ok, record} = Workloads.create(workload())
    Workloads.keep_record(record.id, %{"pullRequest" => %{"url" => @pr}})
    %{gh: gh, workload: Workloads.get(record.id)}
  end

  defp comment(gh) do
    {"POST", @issue <> "/comments", ["body=" <> body]} =
      Enum.find(StandInGitHub.writes(gh), fn {method, endpoint, _} ->
        method == "POST" and endpoint == @issue <> "/comments"
      end)

    body
  end

  defp labels(gh),
    do: for({"POST", @issue <> "/labels", [field]} <- StandInGitHub.writes(gh), do: field)

  @reviews "Reviews:\n- rust: approved\nNo component owns the files: AGENTS.md."

  test "approved reviews: the comment says checks are green and reviews approved, not that the ticket is done",
       %{gh: gh, workload: workload} do
    assert :ok = Tickets.settled(workload, :done, @reviews)

    body = comment(gh)
    assert body =~ "checks are green and its reviews approved it"
    assert body =~ "has not judged whether the ticket's goal is met"
    assert body =~ "for a person, against the ticket"
    refute body =~ "The ticket is done"
    refute body =~ "ticket is done"
    # What each review said, and what nothing looked at, is still there.
    assert body =~ "- rust: approved"
    assert body =~ "No component owns the files: AGENTS.md."
    assert body =~ @pr
  end

  test "the label is still heph:done, and the running label still goes", %{
    gh: gh,
    workload: workload
  } do
    assert :ok = Tickets.settled(workload, :done, @reviews)

    assert labels(gh) == ["labels[]=heph:done"]

    assert Enum.any?(StandInGitHub.writes(gh), fn {method, endpoint, _} ->
             method == "DELETE" and endpoint == @issue <> "/labels/heph%3Arunning"
           end)
  end

  test "no reviews asked for: it says that, and does not say reviews approved", %{
    gh: gh,
    workload: workload
  } do
    assert :ok = Tickets.settled(workload, :done)

    body = comment(gh)
    assert body =~ "checks are green"
    assert body =~ "No review was asked for"
    refute body =~ "reviews approved"
    assert body =~ "has not judged whether the ticket's goal is met"
    refute body =~ "ticket is done"
  end

  test "reviews on but no lens ran: it does not say reviews approved it", %{
    gh: gh,
    workload: workload
  } do
    # What Settle writes when the change selected no lens and none applied to it: the
    # header and what nothing looked at, with no verdict line.
    notes = "Reviews:\nNo lens applied to the components: platform."

    assert :ok = Tickets.settled(workload, :done, notes)

    body = comment(gh)
    refute body =~ "reviews approved"
    assert body =~ "No review was asked for"
    assert body =~ "No lens applied to the components: platform."
    assert body =~ "has not judged whether the ticket's goal is met"
  end

  test "a verdict line is what makes it say reviews approved it", %{gh: gh, workload: workload} do
    notes = "Reviews:\n- spec: approved\nNo lens applied to the components: platform."

    assert :ok = Tickets.settled(workload, :done, notes)

    assert comment(gh) =~ "checks are green and its reviews approved it"
  end

  test "no reviews asked for, with a gate left to CI: both are said", %{
    gh: gh,
    workload: workload
  } do
    left =
      "Left to CI (not run in the workspace):\n- `test`, left to the check `test`, which passed: x"

    assert :ok = Tickets.settled(workload, :done, left)

    body = comment(gh)
    assert body =~ "No review was asked for"
    assert body =~ "Left to CI"
    refute body =~ "reviews approved"
  end

  test "a merge by Hephaestium, or a refusal, is still said", %{gh: gh, workload: workload} do
    assert :ok = Tickets.settled(workload, {:done, :merged}, @reviews)
    assert comment(gh) =~ "Hephaestium merged it."
  end

  test "a merge that was not made says why", %{gh: gh, workload: workload} do
    assert :ok =
             Tickets.settled(workload, {:done, {:not_merged, "it is blocked"}}, @reviews)

    body = comment(gh)
    assert body =~ "Not merged: it is blocked."
    assert body =~ "checks are green and its reviews approved it"
  end
end
