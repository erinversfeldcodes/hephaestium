defmodule ControlPlane.IntakeTest do
  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.{Intake, RunWorkload}
  alias ControlPlane.{StandInGitHub, Workloads}
  alias ControlPlane.Intake, as: Run

  @repo "erinversfeldcodes/gazebolang"
  @approved_at "2026-11-18T07:00:00Z"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-intake-#{System.unique_integer([:positive])}")
    stand_in = StandInGitHub.install(dir)

    previous = Application.get_env(:control_plane, :intake)

    Application.put_env(:control_plane, :intake,
      gh: stand_in.gh,
      heph: stand_in.heph,
      repositories: [@repo]
    )

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      File.rm_rf(dir)
    end)

    # The base case: ticket 12 is labelled by a maintainer, who can write, and
    # nobody has edited it since.
    issue(stand_in, 12)
    labelled(stand_in, 12, [{"maint", @approved_at}])
    permission(stand_in, "maint", "write")
    permission(stand_in, "stranger", "read")
    edits(stand_in, 12, [])
    StandInGitHub.heph_prints(stand_in, built(12, "maint", @approved_at))

    %{gh: stand_in}
  end

  defp issue(gh, number, extra \\ %{}) do
    listed =
      Map.merge(
        %{
          "number" => number,
          "state" => "open",
          "title" => "A ticket",
          "labels" => [%{"name" => "heph:ready"}]
        },
        extra
      )

    StandInGitHub.respond(gh, Run.ready_endpoint(@repo), [listed])
  end

  defp labelled(gh, number, labellings) do
    events =
      for {login, at} <- labellings,
          do: %{
            "event" => "labeled",
            "created_at" => at,
            "actor" => %{"login" => login},
            "label" => %{"name" => "heph:ready"}
          }

    StandInGitHub.respond(gh, Run.events_endpoint(@repo, number), events)
  end

  defp more_events(gh, number, labellings, extra) do
    events =
      for {login, at} <- labellings,
          do: %{
            "event" => "labeled",
            "created_at" => at,
            "actor" => %{"login" => login},
            "label" => %{"name" => "heph:ready"}
          }

    StandInGitHub.respond(gh, Run.events_endpoint(@repo, number), events ++ extra)
  end

  defp permission(gh, login, level),
    do:
      StandInGitHub.respond(gh, Run.permission_endpoint(@repo, login), %{
        "permission" => level,
        "role_name" => level
      })

  defp edits(gh, number, edits) do
    nodes =
      for {login, at} <- edits, do: %{"editedAt" => at, "editor" => login && %{"login" => login}}

    StandInGitHub.respond_graphql(gh, number, %{
      "data" => %{"repository" => %{"issue" => %{"userContentEdits" => %{"nodes" => nodes}}}}
    })
  end

  # The workload `heph workload` would print for a ticket and an approval.
  defp built(number, approver, at, id \\ Ecto.UUID.generate()) do
    workload(id)
    |> put_in(["ticket", "number"], Integer.to_string(number))
    |> put_in(["approval"], %{"approvedBy" => approver, "approvedAt" => at})
  end

  defp results, do: Run.run(@repo)

  test "an approved ticket becomes a queued workload and a job, with the labeller as the approver",
       %{gh: gh} do
    assert [%{ticket: 12, result: :started}] = results()

    [record] = Repo.all(ControlPlane.Workload)
    assert record.repository == @repo
    assert record.ticket_number == 12
    assert record.state == "queued"
    assert record.workload["approval"] == %{"approvedBy" => "maint", "approvedAt" => @approved_at}

    assert [%{args: %{"workload_id" => id}}] = all_enqueued(worker: RunWorkload)
    assert id == record.id

    assert StandInGitHub.heph_calls(gh) == [
             "workload 12 --repo #{@repo} --approver maint --approved-at #{@approved_at}"
           ]
  end

  describe "a ticket that is not approved" do
    test "because the label was applied by someone who cannot write", %{gh: gh} do
      labelled(gh, 12, [{"stranger", @approved_at}])

      assert [%{ticket: 12, result: {:skipped, :approver_cannot_write}}] = results()
      assert Repo.all(ControlPlane.Workload) == []
      assert StandInGitHub.heph_calls(gh) == []
    end

    test "because there is no event that says who applied the label", %{gh: gh} do
      StandInGitHub.respond(gh, Run.events_endpoint(@repo, 12), [])

      assert [%{ticket: 12, result: {:skipped, :no_approval_event}}] = results()
      assert Repo.all(ControlPlane.Workload) == []
    end

    test "because GitHub says the person is not a user at all", %{gh: gh} do
      StandInGitHub.fail(
        gh,
        Run.permission_endpoint(@repo, "maint"),
        ~s({"message":"maint is not a user","status":"404"})
      )

      assert [%{result: {:skipped, :approver_cannot_write}}] = results()
      assert Repo.all(ControlPlane.Workload) == []
    end

    test "when the newest labelling is the one that counts", %{gh: gh} do
      labelled(gh, 12, [{"maint", "2026-11-18T06:00:00Z"}, {"stranger", "2026-11-18T07:00:00Z"}])

      assert [%{result: {:skipped, :approver_cannot_write}}] = results()
    end

    test "for pull requests and closed issues, which are not considered", %{gh: gh} do
      issue(gh, 12, %{"pull_request" => %{"url" => "x"}})
      assert results() == []

      issue(gh, 12, %{"state" => "closed"})
      assert results() == []
      assert Repo.all(ControlPlane.Workload) == []
    end
  end

  describe "an edit after the approval" do
    test "by someone who cannot write voids it", %{gh: gh} do
      edits(gh, 12, [{"stranger", "2026-11-18T08:00:00Z"}])
      assert [%{result: {:skipped, :edited_after_approval}}] = results()
      assert Repo.all(ControlPlane.Workload) == []
    end

    test "by an editor GitHub does not name voids it", %{gh: gh} do
      edits(gh, 12, [{nil, "2026-11-18T08:00:00Z"}])
      assert [%{result: {:skipped, :edited_after_approval}}] = results()
    end

    test "by a maintainer does not", %{gh: gh} do
      edits(gh, 12, [{"maint", "2026-11-18T08:00:00Z"}])
      assert [%{result: :started}] = results()
    end

    test "that was made before the approval does not", %{gh: gh} do
      edits(gh, 12, [{"stranger", "2026-11-18T06:59:59Z"}])
      assert [%{result: :started}] = results()
    end

    test "that renames the ticket follows the same rule", %{gh: gh} do
      renamed = fn login ->
        [
          %{
            "event" => "renamed",
            "created_at" => "2026-11-18T08:00:00Z",
            "actor" => %{"login" => login}
          }
        ]
      end

      more_events(gh, 12, [{"maint", @approved_at}], renamed.("stranger"))
      assert [%{result: {:skipped, :edited_after_approval}}] = results()

      more_events(gh, 12, [{"maint", @approved_at}], renamed.("maint"))
      assert [%{result: :started}] = results()
    end
  end

  describe "a ticket that is blocked by an open ticket" do
    test "is not started, and the result names what blocks it", %{gh: gh} do
      blocked = put_in(built(12, "maint", @approved_at), ["ticket", "blockedBy"], ["5", "9"])
      StandInGitHub.heph_prints(gh, blocked)

      assert [%{ticket: 12, result: {:skipped, {:blocked_by, [5, 9]}}}] = results()

      assert Repo.all(ControlPlane.Workload) == []
      assert all_enqueued(worker: RunWorkload) == []
    end

    test "is started once nothing open blocks it", %{gh: gh} do
      StandInGitHub.heph_prints(gh, built(12, "maint", @approved_at))

      assert [%{ticket: 12, result: :started}] = results()
    end

    test "is started when the list of blockers is empty", %{gh: gh} do
      free = put_in(built(12, "maint", @approved_at), ["ticket", "blockedBy"], [])
      StandInGitHub.heph_prints(gh, free)

      assert [%{ticket: 12, result: :started}] = results()
    end
  end

  test "one approval starts one workload, and a fresh approval starts another", %{gh: gh} do
    assert [%{result: :started}] = results()
    assert [%{result: {:skipped, :already_started}}] = results()
    assert length(Repo.all(ControlPlane.Workload)) == 1

    # The label is removed and applied again, later.
    again = "2026-11-18T09:00:00Z"
    labelled(gh, 12, [{"maint", @approved_at}, {"maint", again}])
    StandInGitHub.heph_prints(gh, built(12, "maint", again))

    assert [%{result: :started}] = results()
    assert length(Repo.all(ControlPlane.Workload)) == 2
  end

  describe "something GitHub or heph cannot say" do
    test "is not an approval, and says why", %{gh: gh} do
      StandInGitHub.fail(gh, Run.events_endpoint(@repo, 12), "gh: HTTP 502 Bad Gateway")
      assert [%{result: {:skipped, {:github_error, message}}}] = results()
      assert message =~ "502"

      File.rm(
        Path.join(
          gh.dir,
          "data/#{String.replace(Run.events_endpoint(@repo, 12), ~r{[/?=&:]}, "_")}.err"
        )
      )

      StandInGitHub.fail(gh, Run.permission_endpoint(@repo, "maint"), "gh: HTTP 500")
      assert [%{result: {:skipped, {:github_error, _}}}] = results()

      File.rm(
        Path.join(
          gh.dir,
          "data/#{String.replace(Run.permission_endpoint(@repo, "maint"), ~r{[/?=&:]}, "_")}.err"
        )
      )

      File.rm!(Path.join(gh.dir, "data/graphql_12.json"))
      assert [%{result: {:skipped, {:github_error, _}}}] = results()

      assert Repo.all(ControlPlane.Workload) == []
    end

    test "heph's warnings on its error output do not spoil the workload it prints", %{gh: gh} do
      StandInGitHub.heph_warns(
        gh,
        "heph: warning: your checkout is at abc, not at GitHub's tip\n"
      )

      assert [%{result: :started}] = results()
      assert [_] = Repo.all(ControlPlane.Workload)
    end

    test "includes heph failing to build the workload", %{gh: gh} do
      StandInGitHub.heph_fails(gh, "heph: #12 in #{@repo} is not open (it is closed)")

      assert [%{result: {:skipped, {:could_not_build, message}}}] = results()
      assert message =~ "is not open"
      assert Repo.all(ControlPlane.Workload) == []
    end

    test "includes a workload that is not for the ticket", %{gh: gh} do
      StandInGitHub.heph_prints(gh, built(13, "maint", @approved_at))

      assert [%{result: {:skipped, {:could_not_build, _}}}] = results()
      assert Repo.all(ControlPlane.Workload) == []
    end
  end

  test "a ticket that is judged after its pull request merges is built so that the merge does not close it" do
    settle = Application.get_env(:control_plane, :settle)
    on_exit(fn -> Application.put_env(:control_plane, :settle, settle) end)
    Application.put_env(:control_plane, :settle, completeness: [@repo])

    assert :ok = perform_job(Intake, %{})

    assert [workload] = Repo.all(ControlPlane.Workload)
    assert workload.workload["judgedAfterMerge"] == true
  end

  test "a ticket that is not judged afterwards is built as it was, so a merge may close it" do
    settle = Application.get_env(:control_plane, :settle)
    on_exit(fn -> Application.put_env(:control_plane, :settle, settle) end)
    Application.put_env(:control_plane, :settle, completeness: [])

    assert :ok = perform_job(Intake, %{})

    assert [workload] = Repo.all(ControlPlane.Workload)
    refute Map.has_key?(workload.workload, "judgedAfterMerge")
  end

  test "the job runs intake for each configured repository" do
    assert :ok = perform_job(Intake, %{})

    assert [_one] = Repo.all(ControlPlane.Workload)
    assert [_one] = all_enqueued(worker: RunWorkload)
    assert Workloads.get(hd(Repo.all(ControlPlane.Workload)).id)
  end
end
