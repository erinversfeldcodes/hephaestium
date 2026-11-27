defmodule ControlPlane.SettleTest do
  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import Ecto.Query

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

  defp run(id, name, status, conclusion, suite \\ nil) do
    run = %{"id" => id, "name" => name, "status" => status, "conclusion" => conclusion}
    if suite, do: Map.put(run, "check_suite", %{"id" => suite}), else: run
  end

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

  test "a check the ruleset does not require counts too: a failing one is failed", %{
    gh: gh,
    id: id
  } do
    StandInGitHub.respond(
      gh,
      @runs,
      runs([run(1, "ci", "completed", "success"), run(2, "lint", "completed", "failure")])
    )

    assert result(id) == {:failed, ["lint"]}
    assert labels(gh) == ["labels[]=heph:failed"]
  end

  test "a check the ruleset does not require waits while it is still running", %{gh: gh, id: id} do
    StandInGitHub.respond(
      gh,
      @runs,
      runs([run(1, "ci", "completed", "success"), run(2, "lint", "in_progress", nil)])
    )

    assert {:waiting, message} = result(id)
    assert message =~ "lint"
    assert StandInGitHub.writes(gh) == []
  end

  test "a check the ruleset does not require is green when it succeeded or was skipped", %{
    gh: gh,
    id: id
  } do
    StandInGitHub.respond(
      gh,
      @runs,
      runs([
        run(1, "ci", "completed", "success"),
        run(2, "lint", "completed", "success"),
        run(3, "preview", "completed", "skipped")
      ])
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

  test "a label the ticket does not have is not a failed write, so the comment is posted once",
       %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @pull, pull(%{"state" => "closed", "merged" => true}))

    StandInGitHub.fail_write(
      gh,
      "DELETE",
      @issue <> "/labels/heph%3Arunning",
      "gh: Label does not exist (HTTP 404)"
    )

    Settle.run()
    Settle.run()

    assert length(comments(gh)) == 1
    assert Workloads.get(id).settled_at
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

  test "a workload that raises while it is settled does not stop the others", %{gh: gh, id: id} do
    # An older one whose record the settling cannot read: its pull request is not a string.
    {:ok, bad} = Workloads.create(workload())
    {:ok, _} = Events.append(bad.id, event(bad.id, 1, started()))
    {:ok, _} = Events.append(bad.id, event(bad.id, 2, finished("OUTCOME_COMPLETED", "")))

    Workloads.keep_record(bad.id, %{
      "outcome" => "OUTCOME_COMPLETED",
      "pullRequest" => %{"url" => %{"x" => 1}}
    })

    ControlPlane.Repo.update_all(
      from(w in ControlPlane.Workload, where: w.id == ^bad.id),
      set: [inserted_at: DateTime.add(DateTime.utc_now(), -3600, :second)]
    )

    results = Settle.run()

    assert %{result: {:error, message}} = Enum.find(results, &(&1.workload_id == bad.id))
    assert message =~ bad.id
    assert Enum.find(results, &(&1.workload_id == id)).result == :done
    assert Workloads.get(id).settled_at
    assert [_] = comments(gh)
    refute Workloads.get(bad.id).settled_at
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

    @annotations "repos/#{@repo}/check-runs/77/annotations"

    test "a check cancelled because its job reached its time limit is a failure, not a re-run",
         %{gh: gh, id: id} do
      reruns(3)

      StandInGitHub.respond(gh, @annotations, [
        %{
          "annotation_level" => "failure",
          "message" => "The job has exceeded the maximum execution time of 15m0s"
        }
      ])

      assert result(id) == {:failed, ["ci"]}
      assert rerun_writes(gh) == [], "a re-run is cold again and times out again"
      assert Workloads.get(id).reruns in [nil, %{}], "and does not spend one"
    end

    test "a cancelled check whose annotations say nothing of a time limit is re-run as before",
         %{gh: gh, id: id} do
      reruns(2)

      StandInGitHub.respond(gh, @annotations, [
        %{"annotation_level" => "warning", "message" => "Node.js 20 actions are deprecated"}
      ])

      assert {:waiting, _} = result(id)
      assert rerun_writes(gh) == ["repos/#{@repo}/actions/jobs/77/rerun"]
    end

    test "annotations that cannot be read leave it a cancellation, re-run as before", %{
      gh: gh,
      id: id
    } do
      reruns(2)
      StandInGitHub.fail(gh, @annotations, "gh: HTTP 500")

      assert {:waiting, _} = result(id)
      assert rerun_writes(gh) == ["repos/#{@repo}/actions/jobs/77/rerun"]
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

    test "a cancelled run beside a live run is waited on, not re-run", %{gh: gh, id: id} do
      reruns(2)

      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(4, "ci", "completed", "cancelled"), run(5, "e2e", "in_progress", nil)])
      )

      assert {:waiting, _} = result(id)
      assert rerun_writes(gh) == []
      assert Workloads.get(id).settled_at == nil
    end

    test "a superseded cancelled run followed by a live run settles on the live run", %{
      gh: gh,
      id: id
    } do
      reruns(2)

      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(9, "ci", "completed", "cancelled"), run(4, "ci-live", "in_progress", nil)])
      )

      assert {:waiting, _} = result(id)
      assert rerun_writes(gh) == []

      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(9, "ci", "completed", "success"), run(4, "ci-live", "completed", "success")])
      )

      assert result(id) == :done
    end

    test "a real failure beside a live run still fails", %{gh: gh, id: id} do
      reruns(2)

      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(4, "ci", "completed", "failure"), run(5, "e2e", "in_progress", nil)])
      )

      assert result(id) == {:failed, ["ci"]}
      assert rerun_writes(gh) == []
    end

    test "a cancelled check the ruleset does not require is re-run like any other", %{
      gh: gh,
      id: id
    } do
      reruns(2)

      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(1, "ci", "completed", "success"), run(5, "lint", "completed", "cancelled")])
      )

      assert {:waiting, _} = result(id)
      assert rerun_writes(gh) == ["repos/#{@repo}/actions/jobs/5/rerun"]
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

    defp record_branch(id, branch) do
      Workloads.keep_record(id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "pullRequest" => %{"url" => @pr_url, "headBranch" => branch}
      })
    end

    test "a revision keeps the lenses and rounds its first run carried, so its push is read too",
         %{id: id} do
      lenses = [%{"pack" => "rust", "document" => "the rust lens"}]
      original = Workloads.get(id)

      Repo.update_all(from(w in ControlPlane.Workload, where: w.id == ^id),
        set: [workload: Map.merge(original.workload, %{"lenses" => lenses, "prepushRounds" => 2})]
      )

      assert result(id) == {:revising, 1}

      revision = revision_of(id)
      assert revision.workload["lenses"] == lenses
      assert revision.workload["prepushRounds"] == 2
    end

    test "a check cancelled by its time limit is revised as a timeout, with the end of its log",
         %{
           gh: gh,
           id: id
         } do
      StandInGitHub.respond(gh, @runs, runs([run(31, "ci", "completed", "cancelled")]))

      StandInGitHub.respond(gh, "repos/#{@repo}/check-runs/31/annotations", [
        %{"message" => "The job has exceeded the maximum execution time of 15m0s"}
      ])

      assert result(id) == {:revising, 1}

      [failure] = revision_of(id).workload["revision"]["failures"]
      assert failure["name"] == "ci"
      assert failure["result"] == "timed_out", "named as what it was, not as a cancellation"
      assert failure["excerpt"] =~ "FAILED: reset spec at reset.rs:40", "what the job was doing"
    end

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

    test "colour codes in a log do not reach the agent", %{gh: gh, id: id} do
      StandInGitHub.respond_text(gh, @logs, "\e[31mFAILED\e[0m: the reset spec\n")

      assert result(id) == {:revising, 1}
      assert [%{"excerpt" => excerpt}] = revision_of(id).workload["revision"]["failures"]
      assert excerpt =~ "FAILED: the reset spec"
      refute excerpt =~ "\e["
    end

    test "a second revision is numbered from the first", %{id: id} do
      {:ok, first} = Workloads.create(revision_workload(id, 1), parent: id)
      complete(first.id)

      Workloads.keep_record(first.id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "pullRequest" => %{"url" => @pr_url}
      })

      Workloads.settle(id)

      assert Enum.find(Settle.run(), &(&1.workload_id == first.id)).result == {:revising, 2}
      assert %{"revision" => %{"attempt" => 2}} = revision_of(first.id).workload
    end

    test "by default nothing is revised", %{id: id} do
      Application.delete_env(:control_plane, :settle)

      assert result(id) == {:failed, ["ci"]}
      assert revision_of(id) == nil
    end

    test "a failed matrix job's test name reaches the revision request", %{gh: gh, id: id} do
      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(31, "ci", "completed", "failure"), run(32, "test (3)", "completed", "failure")])
      )

      StandInGitHub.respond_text(gh, @logs, "Process completed with exit code 1.\n")

      StandInGitHub.respond_text(
        gh,
        "repos/#{@repo}/actions/jobs/32/logs",
        Enum.map_join(1..300, "", &"setup line #{&1}\n") <>
          "  1) test replies (ControlPlane.ApiReplyTest)\n" <>
          "     test/control_plane/api_reply_test.exs:261\n" <>
          "     Assertion with == failed\n" <>
          "945 tests, 1 failure\n" <>
          Enum.map_join(1..1000, "", &"post job cleanup line #{&1}\n")
      )

      assert result(id) == {:revising, 1}
      failures = revision_of(id).workload["revision"]["failures"]
      assert %{"excerpt" => excerpt} = Enum.find(failures, &(&1["name"] == "test (3)"))
      assert excerpt =~ "test/control_plane/api_reply_test.exs:261"
      assert byte_size(excerpt) <= 8_000
    end

    test "a log that cannot be read is reported with its reason and spends no revision", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.fail(gh, @logs, "gh: HTTP 410 Gone")

      assert {:waiting, _} = result(id)
      assert revision_of(id) == nil
      assert [comment] = comments(gh)
      assert comment =~ "ci"
      assert comment =~ "expired"
      assert comment =~ "410"

      assert {:failed, ["ci"], reason} = result(id)
      assert reason =~ "could not be read"
      assert revision_of(id) == nil
      assert Enum.any?(comments(gh), &(&1 =~ "not revised"))
    end

    test "a log that cannot be read at first is read on the next pass", %{gh: gh, id: id} do
      StandInGitHub.fail(gh, @logs, "gh: HTTP 404 Not Found")
      assert {:waiting, _} = result(id)

      StandInGitHub.clear_failure(gh, @logs)

      assert result(id) == {:revising, 1}
      assert [%{"excerpt" => excerpt}] = revision_of(id).workload["revision"]["failures"]
      assert excerpt =~ "FAILED: reset spec at reset.rs:40"
    end

    test "an empty log is unreadable", %{gh: gh, id: id} do
      StandInGitHub.respond_text(gh, @logs, "\n")

      assert {:waiting, _} = result(id)
      assert revision_of(id) == nil
    end

    test "cancelled checks are never revised", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @runs, runs([run(31, "ci", "completed", "cancelled")]))

      assert result(id) == {:failed, ["ci"]}
      assert revision_of(id) == nil
    end

    test "a failed aggregate check of a cancelled duplicate run starts no revision while the other run is live",
         %{gh: gh, id: id} do
      # One push started two runs. GitHub cancelled the first at once: its jobs ended
      # cancelled, and its aggregate job, which fails when a job it needs did not succeed,
      # ended as a failure. The second run is still going.
      StandInGitHub.respond(
        gh,
        @runs,
        runs([
          run(30, "rust", "completed", "cancelled", 1),
          run(31, "ci", "completed", "failure", 1),
          run(40, "rust", "in_progress", nil, 2)
        ])
      )

      assert {:waiting, _} = result(id)
      assert revision_of(id) == nil
      assert StandInGitHub.writes(gh) == []
    end

    test "a failed aggregate of a run that was not cancelled is revised at once beside a live run",
         %{gh: gh, id: id} do
      StandInGitHub.respond(
        gh,
        @runs,
        runs([
          run(30, "rust", "completed", "cancelled", 1),
          run(50, "rust", "completed", "success", 2),
          run(51, "ci", "completed", "failure", 2),
          run(60, "e2e", "in_progress", nil, 2)
        ])
      )

      StandInGitHub.respond_text(gh, "repos/#{@repo}/actions/jobs/51/logs", "FAILED\n")

      assert result(id) == {:revising, 1}
    end

    test "a real failure beside a cancelled duplicate and a live run is revised at once", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.respond(
        gh,
        @runs,
        runs([
          run(30, "rust", "completed", "cancelled", 1),
          run(31, "ci", "completed", "failure", 1),
          run(32, "e2e", "completed", "failure"),
          run(40, "rust", "in_progress", nil)
        ])
      )

      StandInGitHub.respond_text(gh, "repos/#{@repo}/actions/jobs/32/logs", "FAILED\n")

      assert result(id) == {:revising, 1}
      assert [%{"name" => "e2e"}] = revision_of(id).workload["revision"]["failures"]
    end

    test "the stale failed aggregate does not block once the live run succeeded", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.respond(
        gh,
        @runs,
        runs([
          run(30, "rust", "completed", "cancelled"),
          run(31, "ci", "completed", "failure"),
          run(40, "rust", "completed", "success"),
          run(41, "ci", "completed", "success")
        ])
      )

      assert result(id) == :done
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

    test "a real failure of a check the ruleset does not require is revised", %{gh: gh, id: id} do
      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(1, "ci", "completed", "success"), run(31, "preview", "completed", "failure")])
      )

      assert result(id) == {:revising, 1}
      assert [%{"name" => "preview"}] = revision_of(id).workload["revision"]["failures"]
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

    test "a pull request whose head is the recorded branch is revised, whatever the branch is called",
         %{gh: gh, id: id} do
      record_branch(id, "feature/PROJ-12-a-fix")
      StandInGitHub.respond(gh, @pull, pull_on("feature/PROJ-12-a-fix"))

      assert result(id) == {:revising, 1}
      assert revision_of(id).workload["revision"]["branch"] == "feature/PROJ-12-a-fix"
    end

    test "a pull request with the old prefix but another recorded branch is not revised", %{
      gh: gh,
      id: id
    } do
      record_branch(id, "feature/PROJ-12-a-fix")
      StandInGitHub.respond(gh, @pull, pull_on("heph/12-by-a-person"))

      assert {:failed, ["ci"], note} = result(id)
      assert note =~ "recorded"
      assert revision_of(id) == nil
    end

    test "a pull request recorded with no branch and the old prefix is still revised", %{id: id} do
      assert Workloads.recorded_branch(Workloads.get(id)) == nil
      assert result(id) == {:revising, 1}
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

    test "the last failure lists what each revision's agent said, quoted as data", %{id: id} do
      {:ok, first} = Workloads.create(revision_workload(id, 1), parent: id)
      {:ok, _} = Events.append(first.id, event(first.id, 1, started()))

      {:ok, _} =
        Events.append(
          first.id,
          event(
            first.id,
            2,
            agent_message("The failing spec is invite. I changed nothing @someone.")
          )
        )

      {:ok, _} =
        Events.append(first.id, event(first.id, 3, finished("OUTCOME_FAILED", "nothing")))

      Workloads.keep_record(first.id, %{"outcome" => "OUTCOME_FAILED"})
      Workloads.fail(first.id, "the agent changed nothing, so there is nothing to deliver")
      {:ok, second} = Workloads.create(revision_workload(first.id, 2), parent: first.id)
      {:ok, _} = Events.append(second.id, event(second.id, 1, started()))

      {:ok, _} =
        Events.append(second.id, event(second.id, 2, agent_message("I pushed a longer wait.")))

      {:ok, _} = Events.append(second.id, event(second.id, 3, finished("OUTCOME_COMPLETED", "")))

      Workloads.keep_record(second.id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "pullRequest" => %{"url" => @pr_url}
      })

      Workloads.settle(id)

      assert {:failed, ["ci"], note} =
               Enum.find(Settle.run(), &(&1.workload_id == second.id)).result

      assert note =~ "revision 1"
      assert note =~ "The failing spec is invite. I changed nothing @someone."
      assert note =~ "revision 2"
      assert note =~ "I pushed a longer wait."
      assert note =~ "`text\n"
    end

    test "the last failure points to each revision on the dashboard when it is configured, with none of the words",
         %{id: id} do
      Application.put_env(:control_plane, :dashboard, url: "https://heph.example.test")
      on_exit(fn -> Application.delete_env(:control_plane, :dashboard) end)

      {:ok, first} = Workloads.create(revision_workload(id, 1), parent: id)
      {:ok, _} = Events.append(first.id, event(first.id, 1, started()))

      {:ok, _} =
        Events.append(first.id, event(first.id, 2, agent_message("Read lib/secret_plan.ex.")))

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

      assert note =~ "revision 1: failed"
      assert note =~ "https://heph.example.test/tickets/"
      assert note =~ "#run-#{first.id}"
      refute note =~ "secret_plan"
    end

    test "a revision whose agent said nothing is listed as giving no account", %{id: id} do
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

      assert note =~ "revision 1: failed"
      assert note =~ "gave no account"
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

  describe "bringing a green pull request up to date with its base" do
    @behind_branch "heph/12-an-earlier-title-of-the-ticket"
    @compare "repos/#{@repo}/compare/#{@sha}...main"
    @files "repos/#{@repo}/pulls/52/files?per_page=100"

    setup %{gh: gh} do
      previous = Application.get_env(:control_plane, :settle)
      on_exit(fn -> Application.put_env(:control_plane, :settle, previous) end)
      Application.put_env(:control_plane, :settle, revisions: 2)

      StandInGitHub.respond(gh, @pull, behind_pull(@behind_branch))

      # Main gained two commits, one of them in a file the pull request also changes.
      StandInGitHub.respond(gh, @compare, %{
        "commits" => [
          %{
            "sha" => "a1b2c3d4e5f6a7b8",
            "commit" => %{"message" => "chore: bump the toolchain\n\nlong body"}
          },
          %{
            "sha" => "e4f5a6b7c8d9e0f1",
            "commit" => %{"message" => "fix: stop the reset spec racing hydration"}
          }
        ],
        "files" => [
          %{"filename" => "rust-toolchain.toml"},
          %{"filename" => "e2e/tests/account.spec.ts"}
        ]
      })

      StandInGitHub.respond(gh, @files, [
        %{"filename" => "e2e/tests/account.spec.ts"},
        %{"filename" => "crates/web/src/reset.rs"}
      ])

      :ok
    end

    defp behind_pull(ref, repository \\ @repo, state \\ "behind"),
      do:
        pull(%{
          "mergeable_state" => state,
          "head" => %{"sha" => @sha, "ref" => ref, "repo" => %{"full_name" => repository}}
        })

    defp base_revision_of(id),
      do: Enum.find(Workloads.for_ticket(@repo, 12), &(&1.parent_id == id))

    # The pull request was green and settled, and it waits for a person (a required review, a
    # merge). Main moves on while it waits, and nothing looked at it again.
    test "a settled pull request that has fallen behind since is looked at again", %{id: id} do
      Workloads.settle(id)
      assert Workloads.get(id).settled_at

      assert [%{workload_id: ^id, result: :unsettled}] = Settle.stale()
      assert Workloads.get(id).settled_at == nil

      assert result(id) == {:revising, 1}
      assert base_revision_of(id)
    end

    test "a settled pull request that is not behind is left settled", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @pull, behind_pull(@behind_branch, @repo, "clean"))
      Workloads.settle(id)

      assert Settle.stale() == []
      assert Workloads.get(id).settled_at
    end

    test "a settled pull request that is behind and not green is not settled again for it", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.respond(gh, @runs, runs([run(31, "ci", "completed", "failure")]))
      Workloads.settle(id)

      assert Settle.stale() == []
      assert Workloads.get(id).settled_at, "a failure is not announced again on every look"
    end

    test "a settled pull request that is closed is left alone", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @pull, Map.put(behind_pull(@behind_branch), "state", "closed"))
      Workloads.settle(id)

      assert Settle.stale() == []
    end

    test "a settled pull request that is behind with no revision left is left settled", %{id: id} do
      Application.put_env(:control_plane, :settle, revisions: 0)
      Workloads.settle(id)

      assert Settle.stale() == []
      assert Workloads.get(id).settled_at
    end

    test "a settled pull request whose base gained nothing is left settled", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @compare, %{"commits" => [], "files" => []})
      Workloads.settle(id)

      assert Settle.stale() == []
    end

    test "a pull request not yet settled is not touched by looking for stale ones", %{id: id} do
      assert Settle.stale() == []
      assert Workloads.get(id).settled_at == nil
    end

    test "is found by a job" do
      assert :ok = perform_job(ControlPlane.Jobs.KeepCurrent, %{})
    end

    test "a green pull request that is behind is revised, with what came in and no failing check",
         %{gh: gh, id: id} do
      assert result(id) == {:revising, 1}

      revision = base_revision_of(id)
      assert revision.state == "queued"
      assert revision.workload["repository"]["baseCommit"] == @sha

      assert %{
               "pullRequestNumber" => "52",
               "branch" => @behind_branch,
               "attempt" => 1,
               "failures" => [],
               "baseMoved" => moved
             } = revision.workload["revision"]

      assert moved["commits"] == [
               "e4f5a6b fix: stop the reset spec racing hydration",
               "a1b2c3d chore: bump the toolchain"
             ],
             "newest first, a short id and the subject only"

      assert moved["overlappingFiles"] == ["e2e/tests/account.spec.ts"]

      assert Workloads.get(id).settled_at, "the first workload is handed on"
      assert labels(gh) == [], "it is not done: the branch is not up to date"
      assert [comment] = comments(gh)
      assert comment =~ "base branch has moved"
      assert comment =~ "revision 1 of 2"
    end

    test "a pull request that is clean is settled as before", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @pull, behind_pull(@behind_branch, @repo, "clean"))

      assert result(id) == :done
      assert base_revision_of(id) == nil
    end

    test "with revisions off a behind pull request is settled as before", %{id: id} do
      Application.put_env(:control_plane, :settle, revisions: 0)

      assert result(id) == :done
      assert base_revision_of(id) == nil
    end

    test "a pull request that is not on a branch of the runtime's is left alone", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.respond(gh, @pull, behind_pull("feature/someone-elses"))

      assert result(id) == :done
      assert base_revision_of(id) == nil
    end

    test "a pull request from a branch in another repository is left alone", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @pull, behind_pull(@behind_branch, "someone/fork"))

      assert result(id) == :done
      assert base_revision_of(id) == nil
    end

    test "when GitHub shows nothing came in there is nothing to revise", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @compare, %{"commits" => [], "files" => []})

      assert result(id) == :done
      assert base_revision_of(id) == nil
    end

    test "a refused read of what came in waits and starts nothing", %{gh: gh, id: id} do
      StandInGitHub.fail(gh, @compare, "gh: HTTP 502")

      assert {:waiting, _} = result(id)
      assert base_revision_of(id) == nil
      assert comments(gh) == []
    end

    # GitHub runs no checks on a pull request that conflicts with its base, so none is
    # ever reported and waiting for one would be for ever.
    test "a conflicted pull request with no checks is revised to bring the base in", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.respond(gh, @pull, behind_pull(@behind_branch, @repo, "dirty"))
      StandInGitHub.respond(gh, @runs, runs([]))

      assert result(id) == {:revising, 1}

      revision = base_revision_of(id)
      assert revision.state == "queued"

      assert %{"failures" => [], "baseMoved" => moved} = revision.workload["revision"]
      assert moved["overlappingFiles"] == ["e2e/tests/account.spec.ts"]

      assert [comment] = comments(gh)
      assert comment =~ "conflicts with its base"
      assert comment =~ "revision 1 of 2"
    end

    test "a second pass does not start another revision while one is queued", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @pull, behind_pull(@behind_branch, @repo, "dirty"))
      StandInGitHub.respond(gh, @runs, runs([]))

      assert result(id) == {:revising, 1}
      assert Enum.find(Settle.run(), &(&1.workload_id == id)) == nil

      assert [_one] = Enum.filter(Workloads.for_ticket(@repo, 12), &(&1.parent_id == id))
    end

    test "a conflicted pull request with no revision left ends with the conflict named", %{
      gh: gh,
      id: id
    } do
      Application.put_env(:control_plane, :settle, revisions: 1)
      StandInGitHub.respond(gh, @pull, behind_pull(@behind_branch, @repo, "dirty"))
      StandInGitHub.respond(gh, @runs, runs([]))

      # The one revision there was has been made and has ended, pushing nothing that mends it.
      {:ok, first} = Workloads.create(revision_workload(id, 1), parent: id)

      Workloads.keep_record(first.id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "pullRequest" => %{"url" => @pr_url}
      })

      complete(first.id)
      Workloads.settle(id)

      assert {:conflicted, _} = result(first.id)
      assert Workloads.get(first.id).settled_at
      assert [comment] = comments(gh)
      assert comment =~ "conflicts with its base"
    end

    test "a pull request whose mergeability GitHub has not worked out still waits", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.respond(gh, @pull, behind_pull(@behind_branch, @repo, "unknown"))
      StandInGitHub.respond(gh, @runs, runs([]))

      assert {:waiting, _} = result(id)
      assert base_revision_of(id) == nil
    end

    test "a red check is revised for the check, not for the base", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @runs, runs([run(1, "ci", "completed", "failure")]))
      StandInGitHub.respond_text(gh, "repos/#{@repo}/actions/jobs/1/logs", "FAILED\n")

      assert result(id) == {:revising, 1}
      assert %{"failures" => [%{"name" => "ci"}]} = base_revision_of(id).workload["revision"]
      refute Map.has_key?(base_revision_of(id).workload["revision"], "baseMoved")
    end
  end

  describe "judging a ticket once its pull request has merged" do
    @athenaeum_ref "v0.26.0"
    @completeness_skill "repos/erinversfeldcodes/athenaeum/contents/skills/ticket-completeness/SKILL.md?ref=#{@athenaeum_ref}"

    setup %{gh: gh} do
      previous = Application.get_env(:control_plane, :settle)
      on_exit(fn -> Application.put_env(:control_plane, :settle, previous) end)

      Application.put_env(:control_plane, :settle,
        completeness: [@repo],
        athenaeum_ref: @athenaeum_ref
      )

      StandInGitHub.respond(gh, @pull, pull(%{"merged" => true, "state" => "closed"}))
      StandInGitHub.respond(gh, @issue, %{"number" => 12, "state" => "open"})
      StandInGitHub.respond_text(gh, @completeness_skill, "the skill")
      StandInGitHub.heph_prints(gh, workload())
      :ok
    end

    defp evaluation_of(id),
      do:
        Enum.find(
          Workloads.for_ticket(@repo, 12),
          &(&1.id != id and is_map(&1.workload["completeness"]))
        )

    test "a merged pull request whose ticket is still open starts an evaluation", %{id: id} do
      assert result(id) == :done

      evaluation = evaluation_of(id)
      assert evaluation.state == "queued"
      assert evaluation.workload["completeness"]["skillDocument"] == "the skill"
      assert Workloads.get(id).settled_at
    end

    test "nothing is started for a repository that has not opted in", %{id: id} do
      Application.put_env(:control_plane, :settle, athenaeum_ref: @athenaeum_ref)

      assert result(id) == :done
      assert evaluation_of(id) == nil
    end

    test "a pull request that has not merged starts nothing", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @pull, pull())

      assert result(id) == :done
      assert evaluation_of(id) == nil
    end

    test "a ticket that the merge closed starts nothing", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @issue, %{"number" => 12, "state" => "closed"})

      assert result(id) == :done
      assert evaluation_of(id) == nil
    end

    test "an evaluation that cannot be started does not undo the settling", %{gh: gh, id: id} do
      StandInGitHub.fail(gh, @completeness_skill, "gh: HTTP 404")

      assert result(id) == :done
      assert evaluation_of(id) == nil
      assert Workloads.get(id).settled_at, "the ticket is settled and is not tried again"
    end

    test "a merged ticket is evaluated once however often it is settled", %{id: id} do
      assert result(id) == :done
      assert Enum.find(Settle.run(), &(&1.workload_id == id)) == nil

      assert length(
               Enum.filter(Workloads.for_ticket(@repo, 12), &is_map(&1.workload["completeness"]))
             ) == 1
    end
  end

  describe "merging a pull request that is green" do
    @merge "repos/#{@repo}/pulls/52/merge"
    @heph_branch "heph/12-an-earlier-title-of-the-ticket"

    setup %{gh: gh} do
      previous = Application.get_env(:control_plane, :settle)
      on_exit(fn -> Application.put_env(:control_plane, :settle, previous) end)
      Application.put_env(:control_plane, :settle, automerge: [@repo])

      StandInGitHub.respond(gh, @pull, mergeable("clean"))
      :ok
    end

    defp mergeable(state, branch \\ @heph_branch, repository \\ @repo),
      do:
        pull(%{
          "mergeable_state" => state,
          "draft" => state == "draft",
          "head" => %{"sha" => @sha, "ref" => branch, "repo" => %{"full_name" => repository}}
        })

    defp merges(gh),
      do: for({"PUT", @merge, fields} <- StandInGitHub.writes(gh), do: fields)

    defp record_merge_branch(id, branch) do
      Workloads.keep_record(id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "pullRequest" => %{"url" => @pr_url, "headBranch" => branch}
      })
    end

    test "green and clean: it is merged once, squashed, at the head it judged, and the ticket says so",
         %{gh: gh, id: id} do
      assert result(id) == {:done, :merged}

      assert [fields] = merges(gh)
      assert "merge_method=squash" in fields
      assert "sha=#{@sha}" in fields
      assert labels(gh) == ["labels[]=heph:done"]
      assert hd(comments(gh)) =~ "merged"
    end

    test "by default nothing is merged", %{gh: gh, id: id} do
      Application.delete_env(:control_plane, :settle)

      assert result(id) == :done
      assert merges(gh) == []
    end

    test "a repository that is not listed is never merged", %{gh: gh, id: id} do
      Application.put_env(:control_plane, :settle, automerge: ["someone/else"])

      assert result(id) == :done
      assert merges(gh) == []
    end

    test "a pull request GitHub says is blocked is not merged, and nothing is forced", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.respond(gh, @pull, mergeable("blocked"))

      assert {:done, {:not_merged, reason}} = result(id)
      assert reason =~ "blocked"
      assert merges(gh) == []
      assert labels(gh) == ["labels[]=heph:done"]
      assert hd(comments(gh)) =~ "Not merged"
    end

    test "behind, conflicting, unstable and draft pull requests are not merged", %{gh: gh, id: id} do
      for state <- ["behind", "dirty", "unstable", "draft", "has_hooks"] do
        ControlPlane.Repo.update_all(
          from(w in ControlPlane.Workload, where: w.id == ^id),
          set: [settled_at: nil]
        )

        StandInGitHub.respond(gh, @pull, mergeable(state))

        assert {:done, {:not_merged, _}} = result(id), state
        assert merges(gh) == [], state
      end
    end

    test "a pull request whose state GitHub has not worked out yet waits and writes nothing", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.respond(gh, @pull, mergeable("unknown"))

      assert {:waiting, _} = result(id)
      assert StandInGitHub.writes(gh) == []
      assert Workloads.get(id).settled_at == nil
    end

    test "a pull request that is not one the runtime made is not merged", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @pull, mergeable("clean", "feature/someone-elses"))

      assert {:done, {:not_merged, reason}} = result(id)
      assert reason =~ "runtime"
      assert merges(gh) == []
    end

    test "a pull request whose head is the recorded branch is merged, whatever the branch is called",
         %{gh: gh, id: id} do
      record_merge_branch(id, "feature/PROJ-12-a-fix")
      StandInGitHub.respond(gh, @pull, mergeable("clean", "feature/PROJ-12-a-fix"))

      assert result(id) == {:done, :merged}
      assert length(merges(gh)) == 1
    end

    test "a pull request with the old prefix but another recorded branch is not merged", %{
      gh: gh,
      id: id
    } do
      record_merge_branch(id, "feature/PROJ-12-a-fix")

      assert {:done, {:not_merged, reason}} = result(id)
      assert reason =~ "runtime"
      assert merges(gh) == []
    end

    test "a branch from another repository is not merged", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @pull, mergeable("clean", @heph_branch, "someone/fork"))

      assert {:done, {:not_merged, _}} = result(id)
      assert merges(gh) == []
    end

    test "a merge GitHub refuses is reported, settles, and is not tried again", %{gh: gh, id: id} do
      StandInGitHub.fail_write(gh, "PUT", @merge, "gh: Pull Request is not mergeable (HTTP 405)")

      assert {:done, {:not_merged, reason}} = result(id)
      assert reason =~ "not mergeable"
      assert Workloads.get(id).settled_at
      assert hd(comments(gh)) =~ "Not merged"

      assert Enum.find(Settle.run(), &(&1.workload_id == id)) == nil
      assert length(merges(gh)) == 1
    end

    test "a merge that fails for a reason that may pass waits, and is tried at the next pass", %{
      gh: gh,
      id: id
    } do
      StandInGitHub.fail_write(gh, "PUT", @merge, "gh: Bad Gateway (HTTP 502)")

      assert {:waiting, _} = result(id)
      assert labels(gh) == []
      assert Workloads.get(id).settled_at == nil

      File.rm!(Path.join(gh.dir, "data/#{String.replace(@merge, ~r{[/?=&:%]}, "_")}.PUT.err"))
      assert result(id) == {:done, :merged}
    end

    test "checks that are not green are not merged", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @runs, runs([run(1, "ci", "completed", "failure")]))
      assert result(id) == {:failed, ["ci"]}
      assert merges(gh) == []
    end

    test "checks that are still running are not merged", %{gh: gh, id: id} do
      StandInGitHub.respond(
        gh,
        @runs,
        runs([run(1, "ci", "completed", "success"), run(2, "e2e", "in_progress", nil)])
      )

      assert {:waiting, _} = result(id)
      assert merges(gh) == []
    end

    test "a pull request already merged or closed is not merged again", %{gh: gh, id: id} do
      StandInGitHub.respond(gh, @pull, mergeable("clean") |> Map.put("merged", true))

      assert result(id) == :done
      assert merges(gh) == []
    end

    test "a ticket is merged once however often it is settled", %{gh: gh, id: id} do
      assert result(id) == {:done, :merged}
      assert Enum.find(Settle.run(), &(&1.workload_id == id)) == nil
      assert length(merges(gh)) == 1
    end
  end

  describe "one announcement for a head commit and a state" do
    # A ticket keeps a workload for every attempt, and each stays awaiting settlement while it
    # names the same pull request.
    defp more_workloads(count) do
      for _attempt <- 1..count do
        {:ok, record} = Workloads.create(workload())
        wid = record.id
        {:ok, _} = Events.append(wid, event(wid, 1, started()))
        {:ok, _} = Events.append(wid, event(wid, 2, finished("OUTCOME_COMPLETED", "")))

        Workloads.keep_record(wid, %{
          "outcome" => "OUTCOME_COMPLETED",
          "pullRequest" => %{"url" => @pr_url}
        })

        wid
      end
    end

    test "three workloads for one pull request give one comment in a pass, and none in the next",
         %{gh: gh} do
      more_workloads(2)

      Settle.run()
      assert [_one] = comments(gh)

      Settle.run()
      assert [_still_one] = comments(gh)
    end

    test "several check deliveries for a pull request with three unsettled workloads give one comment",
         %{gh: gh, id: id} do
      more_workloads(2)

      # What a delivery that names no change does for each workload awaiting settlement, and
      # what one that names the change does for the latest of the ticket.
      for _delivery <- 1..5 do
        for workload <- Workloads.awaiting_settlement(@repo), do: Settle.one(workload)
        for workload <- Workloads.for_change(@repo, 52), do: Settle.one(workload)
      end

      assert Workloads.get(id).settled_at
      assert [_one] = comments(gh)
    end

    test "a new head commit is announced again", %{gh: gh} do
      Settle.run()
      assert [_one] = comments(gh)

      other = String.duplicate("b", 40)
      StandInGitHub.respond(gh, @pull, pull(%{"head" => %{"sha" => other}}))

      StandInGitHub.respond(
        gh,
        "repos/#{@repo}/commits/#{other}/check-runs?per_page=100",
        runs([run(2, "ci", "completed", "success")])
      )

      [later] = more_workloads(1)
      assert result(later) == :done
      assert length(comments(gh)) == 2
    end
  end
end
