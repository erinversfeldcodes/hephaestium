defmodule ControlPlane.ResumeTest do
  @moduledoc """
  Re-approving a ticket whose pull request is open resumes that pull request, instead of
  starting a workload that cannot deliver (#131). The re-approval is also the maintainer's
  authorisation to spend again on it.
  """

  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import Ecto.Query

  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.RunWorkload
  alias ControlPlane.{Events, Settle, StandInGitHub, Workloads}
  alias ControlPlane.Intake, as: Run

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @pr_url "https://github.com/#{@repo}/pull/52"
  @pull "repos/#{@repo}/pulls/52"
  @sha "f6f2dc5f9df5fb32ff2a5c36ebee4e3bc9e159d5"
  @runs "repos/#{@repo}/commits/#{@sha}/check-runs?per_page=100"
  @rules "repos/#{@repo}/rules/branches/main"
  @branch "heph/12-a-ticket"
  @earlier "2026-11-17T07:00:00Z"
  @approved_at "2026-11-18T07:00:00Z"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-resume-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)
    previous_settle = Application.get_env(:control_plane, :settle)

    Application.put_env(:control_plane, :intake,
      gh: gh.gh,
      heph: gh.heph,
      repositories: [@repo]
    )

    Application.put_env(:control_plane, :settle, revisions: 2)

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      Application.put_env(:control_plane, :settle, previous_settle)
      File.rm_rf(dir)
    end)

    # Ticket 12 was approved on the 17th and worked: its pull request exists. A maintainer
    # labels it ready again on the 18th, and nobody has edited it since.
    listed(gh, ["heph:ready", "heph:failed"])
    labelled(gh, [{"maint", @approved_at}])
    permission(gh, "maint", "write")
    permission(gh, "stranger", "read")
    edits(gh, [])
    StandInGitHub.heph_prints(gh, built("maint", @approved_at))
    pull(gh, %{})

    {:ok, original} = Workloads.create(workload_approved(@earlier))
    completed(original.id, "OUTCOME_COMPLETED", %{"pullRequest" => %{"url" => @pr_url}})
    Workloads.settle(original.id)

    %{gh: gh, original: Workloads.get(original.id)}
  end

  # The ticket as the list of ready tickets shows it.
  defp listed(gh, labels) do
    StandInGitHub.respond(gh, Run.ready_endpoint(@repo), [
      %{
        "number" => 12,
        "state" => "open",
        "title" => "A ticket",
        "labels" => Enum.map(labels, &%{"name" => &1})
      }
    ])
  end

  defp labelled(gh, labellings) do
    events =
      for {login, at} <- labellings,
          do: %{
            "event" => "labeled",
            "created_at" => at,
            "actor" => %{"login" => login},
            "label" => %{"name" => "heph:ready"}
          }

    StandInGitHub.respond(gh, Run.events_endpoint(@repo, 12), events)
  end

  defp permission(gh, login, level),
    do:
      StandInGitHub.respond(gh, Run.permission_endpoint(@repo, login), %{
        "permission" => level,
        "role_name" => level
      })

  defp edits(gh, edits) do
    nodes =
      for {login, at} <- edits, do: %{"editedAt" => at, "editor" => login && %{"login" => login}}

    StandInGitHub.respond_graphql(gh, 12, %{
      "data" => %{"repository" => %{"issue" => %{"userContentEdits" => %{"nodes" => nodes}}}}
    })
  end

  defp built(approver, at) do
    workload()
    |> put_in(["ticket", "number"], "12")
    |> put_in(["approval"], %{"approvedBy" => approver, "approvedAt" => at})
  end

  defp workload_approved(at) do
    workload()
    |> put_in(["ticket", "number"], "12")
    |> put_in(["approval"], %{"approvedBy" => "maint", "approvedAt" => at})
  end

  # The pull request as GitHub has it now.
  defp pull(gh, overrides) do
    StandInGitHub.respond(
      gh,
      @pull,
      Map.merge(
        %{
          "state" => "open",
          "merged" => false,
          "head" => %{"sha" => @sha, "ref" => @branch, "repo" => %{"full_name" => @repo}},
          "base" => %{"ref" => "main"}
        },
        overrides
      )
    )
  end

  defp completed(id, outcome, record) do
    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, finished(outcome, "")))
    Workloads.keep_record(id, Map.put(record, "outcome", outcome))
  end

  # A revision of the workload, with this attempt, that ended as the outcome says.
  defp revision_of(parent, attempt, outcome, record) do
    workload =
      parent.workload
      |> Map.put("id", Ecto.UUID.generate())
      |> Map.put("revision", %{
        "pullRequestNumber" => "52",
        "pullRequestUrl" => @pr_url,
        "branch" => @branch,
        "attempt" => attempt,
        "failures" => [%{"name" => "ci", "result" => "failure", "excerpt" => "x"}]
      })

    {:ok, child} = Workloads.create(workload, parent: parent.id)
    completed(child.id, outcome, record)
    Workloads.settle(child.id)
    Workloads.get(child.id)
  end

  defp writes(gh), do: StandInGitHub.writes(gh)

  defp settled?(id), do: Workloads.get(id).settled_at != nil

  defp count_workloads, do: Repo.aggregate(ControlPlane.Workload, :count)

  test "a re-approved ticket with an open pull request resumes it and starts no workload", %{
    gh: gh,
    original: original
  } do
    before = count_workloads()

    assert [%{ticket: 12, result: :resumed}] = Run.run(@repo)

    assert count_workloads() == before
    assert all_enqueued(worker: RunWorkload) == []
    resumed = Workloads.get(original.id)
    refute resumed.settled_at, "it is looked at again"
    assert resumed.budget_base == 0
    assert DateTime.to_iso8601(resumed.resumed_for) =~ "2026-11-18T07:00:00"
    assert StandInGitHub.writes(gh) != []
  end

  test "the labels end as running, and only the ones the ticket has are removed", %{gh: gh} do
    assert [%{result: :resumed}] = Run.run(@repo)

    posted = for {"POST", @issue <> "/labels", [field]} <- writes(gh), do: field
    removed = for {"DELETE", @issue <> "/labels/" <> label, _} <- writes(gh), do: label

    assert posted == ["labels[]=heph:running"]
    assert Enum.sort(removed) == ["heph%3Afailed", "heph%3Aready"]
  end

  test "a ticket with every state label has each removed", %{gh: gh} do
    listed(gh, ["heph:ready", "heph:failed", "heph:done", "heph:blocked", "bug"])

    assert [%{result: :resumed}] = Run.run(@repo)

    removed = for {"DELETE", @issue <> "/labels/" <> label, _} <- writes(gh), do: label

    assert Enum.sort(removed) ==
             ["heph%3Ablocked", "heph%3Adone", "heph%3Afailed", "heph%3Aready"]

    refute Enum.any?(removed, &(&1 =~ "bug"))
  end

  test "the ticket is told who resumed it and which pull request", %{gh: gh} do
    assert [%{result: :resumed}] = Run.run(@repo)

    assert [{"POST", @issue <> "/comments", ["body=" <> body]}] =
             Enum.filter(writes(gh), fn {m, e, _} ->
               m == "POST" and e == @issue <> "/comments"
             end)

    assert body =~ "resumed"
    assert body =~ "maint"
    assert body =~ @pr_url
  end

  test "an approval resumes once", %{original: original} do
    assert [%{result: :resumed}] = Run.run(@repo)
    Workloads.settle(original.id)

    assert [%{result: {:skipped, :already_started}}] = Run.run(@repo)
    assert settled?(original.id)
  end

  test "a ticket whose workload never opened a pull request starts a new one, as before", %{
    gh: gh,
    original: original
  } do
    # Forget the pull request: what is left is a workload that failed before one existed.
    Repo.update_all(from(w in ControlPlane.Workload, where: w.id == ^original.id),
      set: [record: %{"outcome" => "OUTCOME_FAILED"}, state: "failed"]
    )

    StandInGitHub.heph_prints(gh, built("maint", @approved_at))

    assert [%{result: :started}] = Run.run(@repo)
    assert [_] = all_enqueued(worker: RunWorkload)
  end

  test "a merged pull request is not resumed, and the ticket says why it is skipped", %{gh: gh} do
    pull(gh, %{"state" => "closed", "merged" => true})

    assert [%{result: {:skipped, :pull_request_merged}}] = Run.run(@repo)
    assert writes(gh) == []
  end

  test "a pull request closed without merging is not resumed either", %{gh: gh} do
    pull(gh, %{"state" => "closed", "merged" => false})

    assert [%{result: {:skipped, :pull_request_closed}}] = Run.run(@repo)
    assert writes(gh) == []
  end

  test "a pull request that is not a branch the runtime made is not resumed", %{gh: gh} do
    pull(gh, %{
      "head" => %{"sha" => @sha, "ref" => "feature/theirs", "repo" => %{"full_name" => @repo}}
    })

    assert [%{result: {:skipped, :not_a_runtime_pull_request}}] = Run.run(@repo)
  end

  test "a pull request from another repository is not resumed", %{gh: gh} do
    pull(gh, %{
      "head" => %{"sha" => @sha, "ref" => @branch, "repo" => %{"full_name" => "someone/fork"}}
    })

    assert [%{result: {:skipped, :not_a_runtime_pull_request}}] = Run.run(@repo)
  end

  test "a pull request GitHub cannot be asked about is not resumed", %{gh: gh} do
    StandInGitHub.fail(gh, @pull, "gh: HTTP 502")

    assert [%{result: {:skipped, {:github_error, message}}}] = Run.run(@repo)
    assert message =~ "502"
  end

  test "a re-approval by someone who cannot write resumes nothing", %{gh: gh, original: original} do
    labelled(gh, [{"stranger", @approved_at}])

    assert [%{result: {:skipped, :approver_cannot_write}}] = Run.run(@repo)
    assert settled?(original.id)
    assert writes(gh) == []
  end

  test "an edit by someone who cannot write after the approval resumes nothing", %{
    gh: gh,
    original: original
  } do
    edits(gh, [{"stranger", "2026-11-18T08:00:00Z"}])

    assert [%{result: {:skipped, :edited_after_approval}}] = Run.run(@repo)
    assert settled?(original.id)
  end

  test "the workload resumed is the latest one that completed with the pull request", %{
    original: original
  } do
    revision =
      revision_of(original, 1, "OUTCOME_COMPLETED", %{"pullRequest" => %{"url" => @pr_url}})

    # A later revision that changed nothing has no pull request in its record.
    failed = revision_of(revision, 2, "OUTCOME_FAILED", %{})

    assert [%{result: :resumed}] = Run.run(@repo)

    refute settled?(revision.id)
    assert settled?(original.id)
    assert settled?(failed.id)
    assert Workloads.get(revision.id).budget_base == 1
  end

  describe "settling what was resumed" do
    defp run(id, name, conclusion),
      do: %{"id" => id, "name" => name, "status" => "completed", "conclusion" => conclusion}

    defp checks(gh, runs) do
      StandInGitHub.respond(gh, @rules, [
        %{
          "type" => "required_status_checks",
          "parameters" => %{"required_status_checks" => [%{"context" => "ci"}]}
        }
      ])

      StandInGitHub.respond(gh, @runs, %{"total_count" => length(runs), "check_runs" => runs})
    end

    defp result(id), do: Enum.find(Settle.run(), &(&1.workload_id == id)).result

    test "green checks settle it done again, and no workload is started", %{
      gh: gh,
      original: original
    } do
      checks(gh, [run(1, "ci", "success")])
      assert [%{result: :resumed}] = Run.run(@repo)

      assert result(original.id) == :done

      assert settled?(original.id)
      assert all_enqueued(worker: RunWorkload) == []
      posted = for {"POST", @issue <> "/labels", [field]} <- writes(gh), do: field
      assert "labels[]=heph:done" in posted
    end

    test "a failing check starts a revision, with a budget counted from the resume", %{
      gh: gh,
      original: original
    } do
      # The pull request used its revisions: this is attempt 2 of 2 and it still fails.
      revision =
        revision_of(original, 2, "OUTCOME_COMPLETED", %{"pullRequest" => %{"url" => @pr_url}})

      checks(gh, [run(1, "ci", "failure")])
      assert [%{result: :resumed}] = Run.run(@repo)

      # Without the resume this would fail the ticket for want of revisions.
      assert result(revision.id) == {:revising, 3}

      next = Enum.find(Workloads.for_ticket(@repo, 12), &(&1.parent_id == revision.id))
      assert next.workload["revision"]["attempt"] == 3
      assert next.budget_base == 2, "the new revision inherits where the budget started"

      assert Enum.any?(writes(gh), fn
               {"POST", @issue <> "/comments", ["body=" <> text]} ->
                 text =~ "revision 1 of 2"

               _ ->
                 false
             end)
    end

    test "the budget given by a resume runs out like any other", %{gh: gh, original: original} do
      revision =
        revision_of(original, 2, "OUTCOME_COMPLETED", %{"pullRequest" => %{"url" => @pr_url}})

      checks(gh, [run(1, "ci", "failure")])
      assert [%{result: :resumed}] = Run.run(@repo)
      assert result(revision.id) == {:revising, 3}

      # Revision 3 (the resume's first) pushes and fails again, then revision 4 does too.
      first = Enum.find(Workloads.for_ticket(@repo, 12), &(&1.parent_id == revision.id))
      completed(first.id, "OUTCOME_COMPLETED", %{"pullRequest" => %{"url" => @pr_url}})
      assert result(first.id) == {:revising, 4}

      second = Enum.find(Workloads.for_ticket(@repo, 12), &(&1.parent_id == first.id))
      completed(second.id, "OUTCOME_COMPLETED", %{"pullRequest" => %{"url" => @pr_url}})

      assert {:failed, ["ci"], note} = result(second.id)
      assert note =~ "2 of 2"
    end
  end
end
