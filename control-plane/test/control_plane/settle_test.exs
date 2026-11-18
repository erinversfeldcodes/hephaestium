defmodule ControlPlane.SettleTest do
  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.SettleTickets
  alias ControlPlane.{Events, Settle, StandInGitHub, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @pr_url "https://github.com/#{@repo}/pull/52"
  @sha "f6f2dc5f9df5fb32ff2a5c36ebee4e3bc9e159d5"
  @pull "repos/#{@repo}/pulls/52"
  @rules "repos/#{@repo}/rules/branches/main"
  @runs "repos/#{@repo}/commits/#{@sha}/check-runs?per_page=100"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-settle-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)

    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [])

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      File.rm_rf(dir)
    end)

    {:ok, record} = Workloads.create(workload())
    id = record.id
    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))

    Workloads.keep_record(id, %{
      "outcome" => "OUTCOME_COMPLETED",
      "pullRequest" => %{"url" => @pr_url}
    })

    # The pull request is open, the ruleset names `ci` as required, and every check is green.
    StandInGitHub.respond(gh, @pull, pull())
    StandInGitHub.respond(gh, @rules, [required(["ci"])])
    StandInGitHub.respond(gh, @runs, runs([run(1, "ci", "completed", "success")]))

    %{gh: gh, id: id}
  end

  defp pull(overrides \\ %{}),
    do:
      Map.merge(
        %{
          "state" => "open",
          "merged" => false,
          "head" => %{"sha" => @sha},
          "base" => %{"ref" => "main"}
        },
        overrides
      )

  defp required(contexts),
    do: %{
      "type" => "required_status_checks",
      "parameters" => %{"required_status_checks" => Enum.map(contexts, &%{"context" => &1})}
    }

  defp run(id, name, status, conclusion),
    do: %{"id" => id, "name" => name, "status" => status, "conclusion" => conclusion}

  defp runs(list), do: %{"total_count" => length(list), "check_runs" => list}

  defp labels(gh),
    do: for({"POST", @issue <> "/labels", [field]} <- StandInGitHub.writes(gh), do: field)

  defp removed(gh),
    do: for({"DELETE", @issue <> "/labels/" <> label, _} <- StandInGitHub.writes(gh), do: label)

  defp comments(gh),
    do: for({"POST", @issue <> "/comments", [body]} <- StandInGitHub.writes(gh), do: body)

  defp result(id), do: Enum.find(Settle.run(), &(&1.workload_id == id)).result

  test "every required check green: done, the running label goes, and it says so", %{
    gh: gh,
    id: id
  } do
    assert result(id) == :done

    assert labels(gh) == ["labels[]=heph:done"]
    assert removed(gh) == ["heph%3Arunning"]
    assert [comment] = comments(gh)
    assert comment =~ "green"
    assert comment =~ @pr_url
    assert Workloads.get(id).settled_at
  end

  test "a skipped check counts as green", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @rules, [required(["ci", "e2e"])])

    StandInGitHub.respond(
      gh,
      @runs,
      runs([run(1, "ci", "completed", "success"), run(2, "e2e", "completed", "skipped")])
    )

    assert result(id) == :done
  end

  test "a failed required check: failed, naming the check", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @runs, runs([run(1, "ci", "completed", "failure")]))

    assert result(id) == {:failed, ["ci"]}
    assert labels(gh) == ["labels[]=heph:failed"]
    assert removed(gh) == ["heph%3Arunning"]
    assert hd(comments(gh)) =~ "ci"
  end

  test "a check that is cancelled or timed out is not green", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @rules, [required(["ci", "e2e"])])

    StandInGitHub.respond(
      gh,
      @runs,
      runs([run(1, "ci", "completed", "cancelled"), run(2, "e2e", "completed", "timed_out")])
    )

    assert result(id) == {:failed, ["ci", "e2e"]}
  end

  test "red is not waited on: a failure counts while another check is still running", %{
    gh: gh,
    id: id
  } do
    StandInGitHub.respond(gh, @rules, [required(["ci", "e2e"])])

    StandInGitHub.respond(
      gh,
      @runs,
      runs([run(1, "ci", "completed", "failure"), run(2, "e2e", "in_progress", nil)])
    )

    assert result(id) == {:failed, ["ci"]}
  end

  test "a check still running waits, and writes nothing", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @runs, runs([run(1, "ci", "in_progress", nil)]))

    assert {:waiting, _} = result(id)
    assert StandInGitHub.writes(gh) == []
    assert Workloads.get(id).settled_at == nil
  end

  test "a required check that has not appeared yet waits", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @rules, [required(["ci", "e2e"])])
    StandInGitHub.respond(gh, @runs, runs([run(1, "ci", "completed", "success")]))

    assert {:waiting, _} = result(id)
    assert StandInGitHub.writes(gh) == []
  end

  test "no checks at all is not green", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @runs, runs([]))

    assert {:waiting, _} = result(id)
    assert StandInGitHub.writes(gh) == []
  end

  test "no required checks named and no checks reported is not green either", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @rules, [])
    StandInGitHub.respond(gh, @runs, runs([]))

    assert {:waiting, _} = result(id)
    assert StandInGitHub.writes(gh) == []
  end

  test "a pull request of another repository is not looked at", %{gh: gh, id: id} do
    Workloads.keep_record(id, %{
      "outcome" => "OUTCOME_COMPLETED",
      "pullRequest" => %{"url" => "https://github.com/someone-else/other/pull/52"}
    })

    assert {:waiting, _} = result(id)
    assert StandInGitHub.writes(gh) == []
    assert Workloads.get(id).settled_at == nil
  end

  test "only the newest run of a check counts: a cancelled duplicate does not turn it red", %{
    gh: gh,
    id: id
  } do
    StandInGitHub.respond(
      gh,
      @runs,
      runs([run(9, "ci", "completed", "success"), run(4, "ci", "completed", "failure")])
    )

    assert result(id) == :done
  end

  test "an older green run does not hide a newer failure", %{gh: gh, id: id} do
    StandInGitHub.respond(
      gh,
      @runs,
      runs([run(4, "ci", "completed", "success"), run(9, "ci", "completed", "failure")])
    )

    assert result(id) == {:failed, ["ci"]}
  end

  test "a check the ruleset does not require is not looked at", %{gh: gh, id: id} do
    StandInGitHub.respond(
      gh,
      @runs,
      runs([run(1, "ci", "completed", "success"), run(2, "lint", "completed", "failure")])
    )

    assert result(id) == :done
  end

  test "with no required checks named, every check must be green", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @rules, [])

    StandInGitHub.respond(
      gh,
      @runs,
      runs([run(1, "ci", "completed", "success"), run(2, "lint", "completed", "failure")])
    )

    assert result(id) == {:failed, ["lint"]}
  end

  test "GitHub failing on any of the three reads waits and writes nothing", %{gh: gh, id: id} do
    for endpoint <- [@pull, @rules, @runs] do
      StandInGitHub.respond(gh, @pull, pull())
      StandInGitHub.respond(gh, @rules, [required(["ci"])])
      StandInGitHub.respond(gh, @runs, runs([run(1, "ci", "completed", "success")]))
      StandInGitHub.fail(gh, endpoint, "gh: HTTP 502")

      assert {:waiting, _} = result(id), "reading #{endpoint} failed"
      assert StandInGitHub.writes(gh) == []
      File.rm(Path.join(gh.dir, "data/#{String.replace(endpoint, ~r{[/?=&:%]}, "_")}.err"))
    end
  end

  test "a pull request closed without merging is abandoned", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @pull, pull(%{"state" => "closed"}))

    assert result(id) == :abandoned
    assert labels(gh) == []
    assert removed(gh) == ["heph%3Arunning"]
    assert hd(comments(gh)) =~ "closed"
    assert Workloads.get(id).settled_at
  end

  test "a merged pull request is done", %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @pull, pull(%{"state" => "closed", "merged" => true}))

    assert result(id) == :done
  end

  test "a ticket is settled once", %{gh: gh, id: id} do
    assert result(id) == :done
    writes = length(StandInGitHub.writes(gh))

    assert Enum.find(Settle.run(), &(&1.workload_id == id)) == nil
    assert length(StandInGitHub.writes(gh)) == writes
  end

  test "labels that could not be written leave it unsettled to try again", %{gh: gh, id: id} do
    StandInGitHub.fail_write(gh, "POST", @issue <> "/labels", "gh: HTTP 403")

    assert {:waiting, _} = result(id)
    assert Workloads.get(id).settled_at == nil
  end

  test "workloads that did not complete, or have no pull request, are not looked at", %{gh: gh} do
    {:ok, other} = Workloads.create(workload())
    Workloads.keep_record(other.id, %{"outcome" => "OUTCOME_COMPLETED"})

    ids = Enum.map(Settle.run(), & &1.workload_id)
    refute other.id in ids
    assert Enum.count(StandInGitHub.gh_calls(gh), &String.contains?(&1, "/pulls/")) == 1
  end

  test "the job settles what is awaiting", %{gh: gh, id: id} do
    assert :ok = perform_job(SettleTickets, %{})
    assert labels(gh) == ["labels[]=heph:done"]
    assert Workloads.get(id).settled_at
  end

  describe "re-running cancelled checks" do
    setup %{gh: gh} do
      previous = Application.get_env(:control_plane, :settle)
      on_exit(fn -> Application.put_env(:control_plane, :settle, previous) end)
      StandInGitHub.respond(gh, @runs, runs([run(77, "ci", "completed", "cancelled")]))
      :ok
    end

    defp reruns(n), do: Application.put_env(:control_plane, :settle, reruns: n)

    defp rerun_writes(gh),
      do:
        for(
          {"POST", "repos/" <> _ = endpoint, _} <- StandInGitHub.writes(gh),
          String.ends_with?(endpoint, "/rerun"),
          do: endpoint
        )

    test "by default a cancelled check is failed and nothing is re-run", %{gh: gh, id: id} do
      Application.delete_env(:control_plane, :settle)

      assert result(id) == {:failed, ["ci"]}
      assert rerun_writes(gh) == []
    end

    test "zero re-runs is the same as the default", %{gh: gh, id: id} do
      reruns(0)

      assert result(id) == {:failed, ["ci"]}
      assert rerun_writes(gh) == []
    end

    test "a cancelled check is re-run, and the ticket waits and is not labelled", %{
      gh: gh,
      id: id
    } do
      reruns(2)

      assert {:waiting, message} = result(id)
      assert message =~ "ci"
      assert rerun_writes(gh) == ["repos/#{@repo}/actions/jobs/77/rerun"]
      assert labels(gh) == []
      assert removed(gh) == []
      assert Workloads.get(id).reruns == %{"ci" => 1}
      assert Workloads.get(id).settled_at == nil
    end

    test "the count is kept across passes and stops at the maximum, then the ticket fails", %{
      gh: gh,
      id: id
    } do
      reruns(2)

      assert {:waiting, _} = result(id)
      assert {:waiting, _} = result(id)
      assert length(rerun_writes(gh)) == 2

      assert result(id) == {:failed, ["ci"]}
      assert length(rerun_writes(gh)) == 2
      assert Workloads.get(id).reruns == %{"ci" => 2}
      assert hd(comments(gh)) =~ "re-run 2 times"
    end

    test "a check that failed or timed out is never re-run", %{gh: gh, id: id} do
      reruns(3)
      StandInGitHub.respond(gh, @rules, [required(["ci", "e2e"])])

      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(1, "ci", "completed", "failure"), run(2, "e2e", "completed", "timed_out")])
      )

      assert result(id) == {:failed, ["ci", "e2e"]}
      assert rerun_writes(gh) == []
    end

    test "a real failure beside a cancelled check fails at once without spending a re-run", %{
      gh: gh,
      id: id
    } do
      reruns(3)
      StandInGitHub.respond(gh, @rules, [required(["ci", "e2e"])])

      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(1, "ci", "completed", "failure"), run(2, "e2e", "completed", "cancelled")])
      )

      assert result(id) == {:failed, ["ci", "e2e"]}
      assert rerun_writes(gh) == []
    end

    test "a re-run GitHub refuses waits, and does not count", %{gh: gh, id: id} do
      reruns(2)
      StandInGitHub.fail_write(gh, "POST", "repos/#{@repo}/actions/jobs/77/rerun", "gh: HTTP 403")

      assert {:waiting, _} = result(id)
      assert Workloads.get(id).reruns in [nil, %{}]
      assert Workloads.get(id).settled_at == nil
    end

    test "a cancelled run that a newer run replaced is not re-run", %{gh: gh, id: id} do
      reruns(2)

      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(9, "ci", "completed", "success"), run(4, "ci", "completed", "cancelled")])
      )

      assert result(id) == :done
      assert rerun_writes(gh) == []
    end

    test "a cancelled check the ruleset does not require is not re-run", %{gh: gh, id: id} do
      reruns(2)

      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(1, "ci", "completed", "success"), run(5, "lint", "completed", "cancelled")])
      )

      assert result(id) == :done
      assert rerun_writes(gh) == []
    end
  end

  describe "revising a pull request whose checks really failed" do
    @branch "heph/12-an-earlier-title-of-the-ticket"
    @logs "repos/#{@repo}/actions/jobs/31/logs"

    setup %{gh: gh, id: id} do
      previous = Application.get_env(:control_plane, :settle)
      on_exit(fn -> Application.put_env(:control_plane, :settle, previous) end)
      Application.put_env(:control_plane, :settle, revisions: 2)

      StandInGitHub.respond(gh, @pull, pull_on(@branch))

      StandInGitHub.respond(
        gh,
        @runs,
        runs([
          Map.put(run(31, "ci", "completed", "failure"), "output", %{
            "title" => "1 failed",
            "summary" => "reset_spec failed"
          })
        ])
      )

      StandInGitHub.respond_text(
        gh,
        @logs,
        Enum.map_join(1..1000, "", &"noise line #{&1}\n") <> "FAILED: reset spec at reset.rs:40\n"
      )

      %{revised_from: Workloads.get(id)}
    end

    defp pull_on(ref, repository \\ @repo),
      do:
        pull(%{
          "head" => %{"sha" => @sha, "ref" => ref, "repo" => %{"full_name" => repository}}
        })

    defp revision_of(id),
      do: Enum.find(Workloads.for_ticket(@repo, 12), &(&1.parent_id == id))

    test "a real failure starts a revision from the stored workload, on the head, and says so", %{
      gh: gh,
      id: id,
      revised_from: original
    } do
      assert result(id) == {:revising, 1}

      revision = revision_of(id)
      assert revision.state == "queued"
      assert revision.workload["id"] == revision.id
      assert revision.id != id
      assert revision.workload["repository"]["baseCommit"] == @sha
      assert revision.workload["ticket"] == original.workload["ticket"]
      assert revision.workload["approval"] == original.workload["approval"]

      assert %{
               "pullRequestNumber" => "52",
               "pullRequestUrl" => @pr_url,
               "branch" => @branch,
               "attempt" => 1,
               "failures" => [%{"name" => "ci", "result" => "failure", "excerpt" => excerpt}]
             } = revision.workload["revision"]

      assert excerpt =~ "FAILED: reset spec at reset.rs:40"
      assert excerpt =~ "reset_spec failed"
      assert byte_size(excerpt) <= 8_000
      refute excerpt =~ "noise line 1\n", "the tail of the log, not its start"

      assert_enqueued(
        worker: ControlPlane.Jobs.RunWorkload,
        args: %{"workload_id" => revision.id}
      )

      assert Workloads.get(id).settled_at, "the first workload is handed on"
      assert labels(gh) == []
      assert [comment] = comments(gh)
      assert comment =~ "revision 1 of 2"
      assert comment =~ "ci"
    end

    test "by default nothing is revised", %{id: id} do
      Application.delete_env(:control_plane, :settle)

      assert result(id) == {:failed, ["ci"]}
      assert revision_of(id) == nil
    end

    test "a failure that cannot be read still starts a revision, with what there is", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.fail(gh, @logs, "gh: HTTP 410 Gone")

      assert result(id) == {:revising, 1}
      assert [%{"excerpt" => excerpt}] = revision_of(id).workload["revision"]["failures"]
      assert excerpt =~ "reset_spec failed"
    end

    test "with no output at all the revision says there was none", %{gh: gh, id: id} do
      StandInGitHub.fail(gh, @logs, "gh: HTTP 410 Gone")
      StandInGitHub.respond(gh, @runs, runs([run(31, "ci", "completed", "failure")]))

      assert result(id) == {:revising, 1}
      assert [%{"excerpt" => excerpt}] = revision_of(id).workload["revision"]["failures"]
      assert excerpt =~ "no output"
    end

    test "cancelled checks are never revised", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @runs, runs([run(31, "ci", "completed", "cancelled")]))

      assert result(id) == {:failed, ["ci"]}
      assert revision_of(id) == nil
    end

    test "a real failure beside a cancelled check is revised, for the real one only", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.respond(gh, @rules, [required(["ci", "e2e"])])

      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(31, "ci", "completed", "failure"), run(32, "e2e", "completed", "cancelled")])
      )

      assert result(id) == {:revising, 1}
      assert [%{"name" => "ci"}] = revision_of(id).workload["revision"]["failures"]
    end

    test "timed out is revised", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @runs, runs([run(31, "ci", "completed", "timed_out")]))

      assert result(id) == {:revising, 1}
      assert [%{"result" => "timed_out"}] = revision_of(id).workload["revision"]["failures"]
    end

    test "a pull request that is not on a branch of the runtime's is not revised", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.respond(gh, @pull, pull_on("feature/someone-elses"))

      assert {:failed, ["ci"], note} = result(id)
      assert note =~ "branch"
      assert revision_of(id) == nil
      assert hd(comments(gh)) =~ "not revised"
    end

    test "a pull request from a branch in another repository is not revised", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @pull, pull_on(@branch, "someone/fork"))

      assert {:failed, ["ci"], _} = result(id)
      assert revision_of(id) == nil
    end

    test "the revisions are bounded, and the last failure says what each did", %{gh: gh, id: id} do
      # A chain: the original, a revision that changed nothing, and a revision that pushed.
      {:ok, first} = Workloads.create(revision_workload(id, 1), parent: id)
      Workloads.keep_record(first.id, %{"outcome" => "OUTCOME_FAILED"})
      Workloads.fail(first.id, "the agent changed nothing, so there is nothing to deliver")
      {:ok, second} = Workloads.create(revision_workload(first.id, 2), parent: first.id)

      Workloads.keep_record(second.id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "pullRequest" => %{"url" => @pr_url}
      })

      complete(second.id)
      Workloads.settle(id)

      assert {:failed, ["ci"], note} =
               Enum.find(Settle.run(), &(&1.workload_id == second.id)).result

      assert note =~ "2 of 2"
      assert hd(Enum.reverse(comments(gh))) =~ "the agent changed nothing"
      assert revision_of(second.id) == nil
      assert labels(gh) == ["labels[]=heph:failed"]
    end

    test "a revision that pushed is judged like any workload, and settles when green", %{
      gh: gh,
      id: id
    } do
      assert result(id) == {:revising, 1}
      revision = revision_of(id)

      Workloads.keep_record(revision.id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "pullRequest" => %{"url" => @pr_url}
      })

      complete(revision.id)
      StandInGitHub.respond(gh, @runs, runs([run(40, "ci", "completed", "success")]))

      assert Enum.find(Settle.run(), &(&1.workload_id == revision.id)).result == :done
      assert labels(gh) == ["labels[]=heph:done"]
    end

    test "a refused read of the pull request waits and starts nothing", %{gh: gh, id: id} do
      StandInGitHub.fail(gh, @pull, "gh: HTTP 502")

      assert {:waiting, _} = result(id)
      assert revision_of(id) == nil
    end

    test "settling twice starts one revision", %{id: id} do
      assert result(id) == {:revising, 1}
      assert Enum.find(Settle.run(), &(&1.workload_id == id)) == nil
      assert length(Workloads.for_ticket(@repo, 12)) == 2
    end
  end

  # A workload that ran and completed, as its events say.
  defp complete(id) do
    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))
    :ok
  end

  defp revision_workload(parent_id, attempt) do
    parent = Workloads.get(parent_id)

    parent.workload
    |> Map.put("id", Ecto.UUID.generate())
    |> Map.put("revision", %{
      "pullRequestNumber" => "52",
      "pullRequestUrl" => @pr_url,
      "branch" => "heph/12-an-earlier-title-of-the-ticket",
      "attempt" => attempt,
      "failures" => [%{"name" => "ci", "result" => "failure", "excerpt" => "x"}]
    })
  end
end
