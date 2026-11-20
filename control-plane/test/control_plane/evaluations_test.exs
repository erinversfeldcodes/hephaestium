defmodule ControlPlane.EvaluationsTest do
  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.RunWorkload
  alias ControlPlane.{Evaluations, StandInGitHub, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @ref "v0.26.0"
  @skill "repos/erinversfeldcodes/athenaeum/contents/skills/ticket-completeness/SKILL.md?ref=#{@ref}"
  @tip "f6f2dc5f9df5fb32ff2a5c36ebee4e3bc9e159d5"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-evaluations-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    intake = Application.get_env(:control_plane, :intake)
    settle = Application.get_env(:control_plane, :settle)

    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [])
    Application.put_env(:control_plane, :settle, completeness: [@repo], athenaeum_ref: @ref)

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, intake)
      Application.put_env(:control_plane, :settle, settle)
      File.rm_rf(dir)
    end)

    # The ticket is still open, the skill is at the pinned release, and `heph workload`
    # would build a workload from the ticket as it is now, on the default branch's tip.
    StandInGitHub.respond(gh, @issue, %{"number" => 12, "state" => "open"})
    StandInGitHub.respond_text(gh, @skill, "the ticket-completeness skill")

    fresh =
      workload()
      |> put_in(["ticket", "body"], "the ticket as it reads now")
      |> put_in(["repository", "baseCommit"], @tip)

    StandInGitHub.heph_prints(gh, fresh)

    # The workload whose pull request merged.
    {:ok, merged} = Workloads.create(workload())
    %{gh: gh, merged: Workloads.get(merged.id)}
  end

  defp evaluations(merged),
    do:
      Enum.filter(
        Workloads.for_ticket(@repo, 12),
        &(&1.id != merged.id and is_map(&1.workload["completeness"]))
      )

  test "a merged ticket that is still open is evaluated, on the default branch's tip", %{
    gh: gh,
    merged: merged
  } do
    assert {:started, id} = Evaluations.after_merge(merged)

    assert [evaluation] = evaluations(merged)
    assert evaluation.id == id
    assert evaluation.state == "queued"
    assert evaluation.workload["id"] == id
    assert evaluation.workload["repository"]["baseCommit"] == @tip
    assert evaluation.workload["ticket"]["body"] == "the ticket as it reads now"
    assert evaluation.workload["approval"] == merged.workload["approval"]

    assert evaluation.workload["completeness"] == %{
             "skillDocument" => "the ticket-completeness skill",
             "openItems" => []
           }

    refute Map.has_key?(evaluation.workload, "revision")
    refute Map.has_key?(evaluation.workload, "review")
    assert_enqueued(worker: RunWorkload, args: %{"workload_id" => id})

    approval = merged.workload["approval"]

    assert StandInGitHub.heph_calls(gh) == [
             "workload 12 --repo #{@repo} --approver #{approval["approvedBy"]} --approved-at #{approval["approvedAt"]}"
           ]
  end

  test "nothing is evaluated for a repository that has not opted in", %{merged: merged} do
    Application.put_env(:control_plane, :settle, completeness: [], athenaeum_ref: @ref)

    assert Evaluations.after_merge(merged) == {:skipped, :not_enabled}
    assert evaluations(merged) == []
  end

  test "a ticket that is already closed is not evaluated", %{gh: gh, merged: merged} do
    StandInGitHub.respond(gh, @issue, %{"number" => 12, "state" => "closed"})

    assert Evaluations.after_merge(merged) == {:skipped, :ticket_closed}
    assert evaluations(merged) == []
  end

  test "a ticket already being evaluated is not evaluated again", %{merged: merged} do
    assert {:started, _} = Evaluations.after_merge(merged)

    assert Evaluations.after_merge(merged) == {:skipped, :already_evaluating}
    assert length(evaluations(merged)) == 1
  end

  test "without a pinned release there is no skill to read, and nothing starts", %{
    merged: merged
  } do
    Application.put_env(:control_plane, :settle, completeness: [@repo], athenaeum_ref: "")

    assert {:skipped, {:no_skill, message}} = Evaluations.after_merge(merged)
    assert message =~ "HEPH_ATHENAEUM_REF"
    assert evaluations(merged) == []
  end

  test "a skill that cannot be read starts nothing", %{gh: gh, merged: merged} do
    StandInGitHub.fail(gh, @skill, "gh: HTTP 404")

    assert {:skipped, {:no_skill, _}} = Evaluations.after_merge(merged)
    assert evaluations(merged) == []
  end

  test "a ticket that cannot be read starts nothing", %{gh: gh, merged: merged} do
    StandInGitHub.fail(gh, @issue, "gh: HTTP 502")

    assert {:skipped, {:github_error, _}} = Evaluations.after_merge(merged)
    assert evaluations(merged) == []
  end

  test "a workload that heph cannot build starts nothing", %{gh: gh, merged: merged} do
    File.write!(Path.join(gh.dir, "heph.err"), "heph: no such issue")

    assert {:skipped, {:could_not_build, message}} = Evaluations.after_merge(merged)
    assert message =~ "no such issue"
    assert evaluations(merged) == []
  end
end
