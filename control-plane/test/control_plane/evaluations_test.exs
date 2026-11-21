defmodule ControlPlane.EvaluationsTest do
  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import Ecto.Query

  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.RunWorkload
  alias ControlPlane.{Evaluations, Events, StandInGitHub, Workloads}

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

  describe "which verdicts close their ticket" do
    @met_requirement %{
      "text" => "the spec waits",
      "settled_by" => "evidence",
      "status" => "met",
      "evidence" => %{"kind" => "test", "where" => "e2e/a.spec.ts"}
    }

    defp deciding(overrides),
      do:
        Map.merge(
          %{
            "verdict" => "met",
            "decides" => "evidence",
            "epic" => false,
            "requirements" => [@met_requirement],
            "owed" => [],
            "could_not_check" => []
          },
          overrides
        )

    test "a verdict the evidence decides closes it" do
      assert Evaluations.closes?(deciding(%{}))
    end

    test "observed evidence closes it as a test does" do
      observed = put_in(@met_requirement, ["evidence", "kind"], "observed")
      assert Evaluations.closes?(deciding(%{"requirements" => [observed]}))
    end

    test "nothing else does" do
      shaky = fn changes -> Map.merge(@met_requirement, changes) end

      for {why, verdict} <- [
            {"not met", deciding(%{"verdict" => "not_met"})},
            {"owed", deciding(%{"verdict" => "owed"})},
            {"cannot judge", deciding(%{"verdict" => "cannot_judge"})},
            {"a person decides", deciding(%{"decides" => "person"})},
            {"an epic", deciding(%{"epic" => true})},
            {"no requirements", deciding(%{"requirements" => []})},
            {"something owed", deciding(%{"owed" => [%{"what" => "x"}]})},
            {"something not checked", deciding(%{"could_not_check" => ["the logs"]})},
            {"a requirement a person settles",
             deciding(%{"requirements" => [shaky.(%{"settled_by" => "person"})]})},
            {"a requirement not met",
             deciding(%{"requirements" => [shaky.(%{"status" => "owed"})]})},
            {"evidence that is only structure",
             deciding(%{
               "requirements" => [put_in(@met_requirement, ["evidence", "kind"], "structure")]
             })},
            {"evidence that is only a claim",
             deciding(%{
               "requirements" => [put_in(@met_requirement, ["evidence", "kind"], "claim")]
             })},
            {"no evidence",
             deciding(%{"requirements" => [Map.delete(@met_requirement, "evidence")]})}
          ] do
        refute Evaluations.closes?(verdict), "closed on a verdict with #{why}"
      end
    end

    test "a repository opts in to closing, apart from opting in to evaluation" do
      Application.put_env(:control_plane, :settle, completeness: [@repo], closures: [])
      refute Evaluations.closing?(@repo)

      Application.put_env(:control_plane, :settle, completeness: [@repo], closures: [@repo])
      assert Evaluations.closing?(@repo)
    end
  end

  describe "a ticket judged too often" do
    defp judged_times(merged, count, age_hours \\ 0) do
      for _ <- 1..count do
        {:ok, record} =
          Workloads.create(Map.put(workload(), "completeness", %{"skillDocument" => "x"}))

        {:ok, _} = Events.append(record.id, event(record.id, 1, started()))

        {:ok, _} =
          Events.append(record.id, event(record.id, 2, finished("OUTCOME_COMPLETED", "")))

        then = DateTime.add(DateTime.utc_now(), -age_hours * 3600, :second)

        ControlPlane.Repo.update_all(
          Ecto.Query.from(w in ControlPlane.Workload, where: w.id == ^record.id),
          set: [inserted_at: then]
        )
      end

      merged
    end

    test "is not judged again within a day once it has been judged six times", %{merged: merged} do
      judged_times(merged, 6)

      assert Evaluations.after_merge(merged) == {:skipped, :judged_too_often}

      assert length(
               Enum.filter(Workloads.for_ticket(@repo, 12), &is_map(&1.workload["completeness"]))
             ) == 6
    end

    test "is judged when it has been judged five times", %{merged: merged} do
      judged_times(merged, 5)

      assert {:started, _} = Evaluations.after_merge(merged)
    end

    test "counts only the last day", %{merged: merged} do
      judged_times(merged, 6, 30)

      assert {:started, _} = Evaluations.after_merge(merged)
    end

    test "is counted for each approval, so a person's new approval is not held by an old one's",
         %{merged: merged} do
      judged_times(merged, 6)
      approval = merged.workload["approval"]

      # The same approval again is held.
      assert Evaluations.after_merge(merged, %{
               login: approval["approvedBy"],
               text: approval["approvedAt"]
             }) == {:skipped, :judged_too_often}

      # A new approval is a person's act, and starts its own count.
      assert {:started, _} =
               Evaluations.after_merge(merged, %{
                 login: approval["approvedBy"],
                 text: "2026-11-21T04:30:00Z"
               })
    end

    test "a new approval is itself held after six evaluations for it", %{gh: gh, merged: merged} do
      approval = merged.workload["approval"]
      newer = %{login: approval["approvedBy"], text: "2026-11-21T04:30:00Z"}
      judged_times(merged, 6)

      # `heph` prints the approval it was given, so what it builds carries the new one.
      builds = fn ->
        StandInGitHub.heph_prints(
          gh,
          workload()
          |> put_in(["approval", "approvedAt"], newer.text)
          |> Map.put("id", Ecto.UUID.generate())
        )
      end

      builds.()
      assert {:started, id} = Evaluations.after_merge(merged, newer)
      {:ok, _} = Events.append(id, event(id, 1, started()))
      {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))

      # Four more under the new approval: with the one started, five.
      for _ <- 1..4 do
        {:ok, record} =
          Workloads.create(
            workload()
            |> put_in(["approval", "approvedAt"], newer.text)
            |> Map.put("completeness", %{"skillDocument" => "x"})
          )

        {:ok, _} = Events.append(record.id, event(record.id, 1, started()))

        {:ok, _} =
          Events.append(record.id, event(record.id, 2, finished("OUTCOME_COMPLETED", "")))
      end

      builds.()
      assert {:started, id} = Evaluations.after_merge(merged, newer)
      {:ok, _} = Events.append(id, event(id, 1, started()))
      {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))

      # Six now, under this approval.
      builds.()
      assert Evaluations.after_merge(merged, newer) == {:skipped, :judged_too_often}
    end
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

  describe "judging a ticket again while its verdict is owed" do
    # The evaluation finished at 07:00:02 on the 18th.
    @finished ~U[2026-11-18 07:00:02Z]

    @owed %{
      "what" => "the cache hit rate after the change",
      "run" => "gh cache list",
      "when" => "after ten runs",
      "settled_by" => "no lower than before"
    }

    defp verdict(kind, owed \\ [@owed]),
      do: %{
        "schema_version" => 0.0,
        "ticket" => 12.0,
        "commit" => @tip,
        "verdict" => kind,
        "decides" => "person",
        "decides_because" => "a requirement is owed",
        "epic" => false,
        "requirements" => [],
        "goal" => %{"status" => "owed", "why" => "later"},
        "owed" => owed,
        "not_filed" => [],
        "could_not_check" => []
      }

    # An evaluation that has ended, with the verdict it kept.
    defp evaluated(verdict, outcome \\ "OUTCOME_COMPLETED") do
      built = Map.put(workload(), "completeness", %{"skillDocument" => "x", "openItems" => []})
      {:ok, record} = Workloads.create(built)
      {:ok, _} = Events.append(record.id, event(record.id, 1, started()))
      {:ok, _} = Events.append(record.id, event(record.id, 2, finished(outcome, "")))
      Workloads.keep_record(record.id, %{"outcome" => outcome, "completeness" => verdict})
      Workloads.get(record.id)
    end

    defp later(hours), do: DateTime.add(@finished, hours * 3600, :second)

    defp all_evaluations,
      do: Enum.filter(Workloads.for_ticket(@repo, 12), &is_map(&1.workload["completeness"]))

    test "an owed verdict is judged again once a day has passed, told what is open", %{
      gh: gh
    } do
      first = evaluated(verdict("owed"))
      StandInGitHub.heph_prints(gh, Map.put(workload(), "id", Ecto.UUID.generate()))

      assert [{:started, id}] = Evaluations.recheck(later(25))

      again = Enum.find(all_evaluations(), &(&1.id == id))
      assert again.id != first.id

      assert again.workload["completeness"]["openItems"] == [
               %{
                 "what" => "the cache hit rate after the change",
                 "run" => "gh cache list",
                 "when" => "after ten runs",
                 "settledBy" => "no lower than before"
               }
             ]
    end

    test "nothing is judged again before the interval has passed" do
      evaluated(verdict("owed"))

      assert Evaluations.recheck(later(2)) == []
      assert length(all_evaluations()) == 1
    end

    test "a verdict that is not owed is not judged again" do
      for kind <- ["met", "not_met", "cannot_judge"] do
        evaluated(verdict(kind, []))
      end

      assert Evaluations.recheck(later(48)) == []
    end

    test "an evaluation that failed is judged again once the interval has passed, with nothing taken from it",
         %{gh: gh} do
      evaluated(verdict("owed"), "OUTCOME_FAILED")
      StandInGitHub.heph_prints(gh, Map.put(workload(), "id", Ecto.UUID.generate()))

      assert [{:started, id}] = Evaluations.recheck(later(25))

      again = Enum.find(all_evaluations(), &(&1.id == id))
      assert again.workload["completeness"]["openItems"] == []
    end

    test "an evaluation that failed is judged again within the hour, not waiting a day for what it was not owed",
         %{gh: gh} do
      evaluated(verdict("owed"), "OUTCOME_FAILED")
      StandInGitHub.heph_prints(gh, Map.put(workload(), "id", Ecto.UUID.generate()))

      assert [{:started, _}] = Evaluations.recheck(later(2))
    end

    test "an evaluation that failed is not judged again the moment it ended" do
      evaluated(verdict("owed"), "OUTCOME_FAILED")

      assert Evaluations.recheck(later(0)) == []
    end

    test "an evaluation that failed is judged again only as often as the limit allows" do
      Application.put_env(:control_plane, :settle,
        completeness: [@repo],
        athenaeum_ref: @ref,
        completeness_rechecks: 1
      )

      evaluated(verdict("owed"), "OUTCOME_FAILED")
      evaluated(verdict("owed"), "OUTCOME_FAILED")

      assert Evaluations.recheck(later(48)) == []
    end

    test "the number of times is limited, and the first judgement is not counted against it" do
      Application.put_env(:control_plane, :settle,
        completeness: [@repo],
        athenaeum_ref: @ref,
        completeness_rechecks: 1
      )

      evaluated(verdict("owed"))
      evaluated(verdict("owed"))

      # Two evaluations already: the first and one recheck.
      assert Evaluations.recheck(later(48)) == []
      assert length(all_evaluations()) == 2
    end

    test "a ticket that has been closed since is left alone", %{gh: gh} do
      evaluated(verdict("owed"))
      StandInGitHub.respond(gh, @issue, %{"number" => 12, "state" => "closed"})

      assert Evaluations.recheck(later(25)) == [{:skipped, :ticket_closed}]
      assert length(all_evaluations()) == 1
    end

    test "an evaluation still under way is not joined by another", %{merged: merged} do
      evaluated(verdict("owed"))
      assert {:started, _} = Evaluations.after_merge(merged)

      assert Evaluations.recheck(later(25)) == []
      assert length(all_evaluations()) == 2, "the one under way is the latest, and is waited for"
    end

    test "a repository that has not opted in is not looked at" do
      evaluated(verdict("owed"))
      Application.put_env(:control_plane, :settle, completeness: [], athenaeum_ref: @ref)

      assert Evaluations.recheck(later(48)) == []
    end

    test "the job runs the recheck" do
      assert :ok = perform_job(ControlPlane.Jobs.RecheckOwed, %{})
    end
  end

  describe "an epic whose sub-issues are all closed" do
    @listing "repos/#{@repo}/issues?state=open&per_page=100"
    @epic_issue "repos/#{@repo}/issues/20"
    @sub_issues "repos/#{@repo}/issues/20/sub_issues"

    defp epic(summary, extra \\ %{}),
      do:
        Map.merge(
          %{"number" => 20, "state" => "open", "sub_issues_summary" => summary},
          extra
        )

    defp all_closed, do: %{"total" => 2, "completed" => 2, "percent_completed" => 100}

    # Ticket 12 is a sub-issue that was closed at the given time, and was worked by heph.
    defp epic_is(gh, issue, closed_at \\ "2026-11-20T10:00:00Z") do
      StandInGitHub.respond(gh, @listing, [issue])
      StandInGitHub.respond(gh, @epic_issue, %{"number" => 20, "state" => "open"})

      StandInGitHub.respond(gh, @sub_issues, [
        %{"number" => 12, "state" => "closed", "closed_at" => closed_at},
        %{"number" => 13, "state" => "closed", "closed_at" => "2026-11-19T10:00:00Z"}
      ])

      StandInGitHub.heph_prints(
        gh,
        workload() |> put_in(["ticket", "number"], "20")
      )
    end

    defp epic_evaluations,
      do: Enum.filter(Workloads.for_ticket(@repo, 20), &is_map(&1.workload["completeness"]))

    test "is judged, for the approval of the work beneath it", %{gh: gh, merged: merged} do
      epic_is(gh, epic(all_closed()))

      assert [{:started, id}] = Evaluations.epics()

      assert [evaluation] = epic_evaluations()
      assert evaluation.id == id

      approval = merged.workload["approval"]

      assert Enum.any?(
               StandInGitHub.heph_calls(gh),
               &(&1 ==
                   "workload 20 --repo #{@repo} --approver #{approval["approvedBy"]} --approved-at #{approval["approvedAt"]}")
             )
    end

    test "is not judged while a sub-issue is still open", %{gh: gh} do
      epic_is(gh, epic(%{"total" => 2, "completed" => 1, "percent_completed" => 50}))

      assert Evaluations.epics() == []
      assert epic_evaluations() == []
    end

    test "is not judged when it has no sub-issues", %{gh: gh} do
      epic_is(gh, epic(%{"total" => 0, "completed" => 0, "percent_completed" => 0}))

      assert Evaluations.epics() == []
    end

    test "is not mistaken for a pull request", %{gh: gh} do
      epic_is(gh, epic(all_closed(), %{"pull_request" => %{"url" => "x"}}))

      assert Evaluations.epics() == []
    end

    test "is judged once, until another sub-issue closes after that", %{gh: gh} do
      epic_is(gh, epic(all_closed()), "2026-11-01T10:00:00Z")

      assert [{:started, id}] = Evaluations.epics()
      # The judgement has ended, so only the rule can stop another.
      {:ok, _} = Events.append(id, event(id, 1, started()))
      {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))

      assert Evaluations.epics() == []
      assert length(epic_evaluations()) == 1

      # A sub-issue closes in the future, after the judgement.
      epic_is(gh, epic(all_closed()), "2099-01-01T00:00:00Z")
      assert [{:started, _}] = Evaluations.epics()
    end

    test "is not looked at for a repository that has not opted in", %{gh: gh} do
      epic_is(gh, epic(all_closed()))
      Application.put_env(:control_plane, :settle, completeness: [], athenaeum_ref: @ref)

      assert Evaluations.epics() == []
    end

    test "is found by a job" do
      assert :ok = perform_job(ControlPlane.Jobs.RollUpEpics, %{})
    end
  end

  describe "a pull request that merged after its ticket was settled" do
    @pull "repos/#{@repo}/pulls/52"

    # A workload that completed and was settled (its pull request was green), as it is
    # when a person merges the pull request later.
    defp settled(gh, pull) do
      {:ok, record} = Workloads.create(workload())
      {:ok, _} = Events.append(record.id, event(record.id, 1, started()))
      {:ok, _} = Events.append(record.id, event(record.id, 2, finished("OUTCOME_COMPLETED", "")))

      Workloads.keep_record(record.id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "pullRequest" => %{"url" => "https://github.com/#{@repo}/pull/52"}
      })

      Workloads.settle(record.id)
      StandInGitHub.respond(gh, @pull, pull)
      Workloads.get(record.id)
    end

    defp the_evaluations,
      do: Enum.filter(Workloads.for_ticket(@repo, 12), &is_map(&1.workload["completeness"]))

    test "is judged once it is seen to have merged", %{gh: gh} do
      settled(gh, %{"state" => "closed", "merged" => true})

      assert [{:started, _}] = Evaluations.merged()
      assert length(the_evaluations()) == 1
    end

    test "is not judged while it is still open", %{gh: gh} do
      settled(gh, %{"state" => "open", "merged" => false})

      assert Evaluations.merged() == []
      assert the_evaluations() == []
    end

    test "is not judged when it was closed without merging", %{gh: gh} do
      settled(gh, %{"state" => "closed", "merged" => false})

      assert Evaluations.merged() == []
      assert the_evaluations() == []
    end

    test "is judged once, however often it is looked at", %{gh: gh} do
      settled(gh, %{"state" => "closed", "merged" => true})

      assert [{:started, _}] = Evaluations.merged()
      assert Evaluations.merged() == []
      assert length(the_evaluations()) == 1
    end

    test "is not judged for a ticket the merge closed", %{gh: gh} do
      settled(gh, %{"state" => "closed", "merged" => true})
      StandInGitHub.respond(gh, @issue, %{"number" => 12, "state" => "closed"})

      assert Evaluations.merged() == [{:skipped, :ticket_closed}]
      assert the_evaluations() == []
    end

    test "is not looked at for a repository that has not opted in", %{gh: gh} do
      settled(gh, %{"state" => "closed", "merged" => true})
      Application.put_env(:control_plane, :settle, completeness: [], athenaeum_ref: @ref)

      assert Evaluations.merged() == []
    end

    test "is not looked at once it was settled long ago", %{gh: gh} do
      old = settled(gh, %{"state" => "closed", "merged" => true})

      ControlPlane.Repo.update_all(
        from(w in ControlPlane.Workload, where: w.id == ^old.id),
        set: [updated_at: DateTime.add(DateTime.utc_now(), -30 * 86_400, :second)]
      )

      assert Evaluations.merged() == []
    end

    test "a read of the pull request that fails waits, and starts nothing", %{gh: gh} do
      settled(gh, %{"state" => "closed", "merged" => true})
      StandInGitHub.fail(gh, @pull, "gh: HTTP 502")

      assert [{:skipped, {:github_error, _}}] = Evaluations.merged()
      assert the_evaluations() == []
    end

    test "the job looks for them" do
      assert :ok = perform_job(ControlPlane.Jobs.EvaluateMerged, %{})
    end
  end
end
