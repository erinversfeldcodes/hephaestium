defmodule ControlPlane.GapsTest do
  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.RunWorkload
  alias ControlPlane.{Events, Gaps, StandInGitHub, Tickets, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @ref "v0.26.1"
  @skill "repos/erinversfeldcodes/athenaeum/contents/skills/close-the-gap/SKILL.md?ref=#{@ref}"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-gaps-#{System.unique_integer([:positive])}")
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

    StandInGitHub.respond(gh, @issue, %{"number" => 12, "state" => "open"})
    StandInGitHub.respond_text(gh, @skill, "the close-the-gap skill")
    StandInGitHub.heph_prints(gh, workload())

    # The workload the ticket was worked by, which carries who approved it.
    {:ok, worked} = Workloads.create(workload())
    %{gh: gh, worked: Workloads.get(worked.id)}
  end

  defp steps,
    do: Enum.filter(Workloads.for_ticket(@repo, 12), &is_map(&1.workload["gap"]))

  defp comments(gh),
    do: for({"POST", @issue <> "/comments", [body]} <- StandInGitHub.writes(gh), do: body)

  defp label_writes(gh),
    do: for({_, @issue <> "/labels" <> _, _} = write <- StandInGitHub.writes(gh), do: write)

  # A step that ended, with what it kept.
  defp ended_step(outcome, reason, kept, at \\ nil) do
    gap = %{"skillDocument" => "x", "because" => "a person replied"}
    {:ok, record} = Workloads.create(Map.put(workload(), "gap", gap))
    {:ok, _} = Events.append(record.id, event(record.id, 1, started(), at))
    {:ok, _} = Events.append(record.id, event(record.id, 2, finished(outcome, reason), at))
    Workloads.keep_record(record.id, Map.merge(%{"outcome" => outcome}, kept))
    Workloads.get(record.id)
  end

  describe "starting a step" do
    test "queues a read-only step built from the ticket as it reads now", %{
      gh: gh,
      worked: worked
    } do
      assert {:started, id} = Gaps.start(@repo, 12, "the verdict found the goal not met")

      assert [step] = steps()
      assert step.id == id
      assert step.state == "queued"

      assert step.workload["gap"] == %{
               "skillDocument" => "the close-the-gap skill",
               "because" => "the verdict found the goal not met"
             }

      assert step.workload["approval"] == worked.workload["approval"]
      refute Map.has_key?(step.workload, "completeness")
      assert_enqueued(worker: RunWorkload, args: %{"workload_id" => id})

      approval = worked.workload["approval"]

      assert StandInGitHub.heph_calls(gh) == [
               "workload 12 --repo #{@repo} --approver #{approval["approvedBy"]} --approved-at #{approval["approvedAt"]}"
             ]
    end

    test "is not started for a repository that has not opted in" do
      Application.put_env(:control_plane, :settle, completeness: [], athenaeum_ref: @ref)

      assert Gaps.start(@repo, 12, "x") == {:skipped, :not_enabled}
      assert steps() == []
    end

    test "is not started for a ticket that is closed", %{gh: gh} do
      StandInGitHub.respond(gh, @issue, %{"number" => 12, "state" => "closed"})

      assert Gaps.start(@repo, 12, "x") == {:skipped, :ticket_closed}
    end

    test "is not started while another step of the ticket is under way" do
      assert {:started, _} = Gaps.start(@repo, 12, "x")

      assert Gaps.start(@repo, 12, "x") == {:skipped, :already_stepping}
      assert length(steps()) == 1
    end

    test "is not started for a ticket nobody approved", %{gh: gh} do
      Ecto.Adapters.SQL.query!(ControlPlane.Repo, "delete from workloads", [])
      StandInGitHub.heph_prints(gh, workload())

      assert Gaps.start(@repo, 12, "x") == {:skipped, :no_approval}
    end

    test "is started at most six times a day, however it was asked for" do
      for _ <- 1..6 do
        ended_step("OUTCOME_COMPLETED", "", %{"gap" => %{"step" => "question", "text" => "q"}})
      end

      assert Gaps.start(@repo, 12, "x") == {:skipped, :asked_too_often}
    end
  end

  describe "a step that ends" do
    test "a question is commented on the ticket as written, and no label changes", %{gh: gh} do
      step =
        ended_step("OUTCOME_COMPLETED", "", %{
          "gap" => %{"step" => "question", "text" => "## Next question\n\nWhich button?"}
        })

      assert :ok = Tickets.ended(step)

      assert [comment] = comments(gh)
      assert comment =~ "**Hephaestium:**"
      assert comment =~ "## Next question\n\nWhich button?"
      assert label_writes(gh) == []
    end

    test "starting a step changes no label", %{gh: gh} do
      {:ok, record} = Workloads.create(Map.put(workload(), "gap", %{"skillDocument" => "x"}))

      assert :ok = Tickets.started(Workloads.get(record.id))
      assert label_writes(gh) == []
    end

    test "a step that failed says so, and a person's next reply starts another", %{gh: gh} do
      step = ended_step("OUTCOME_FAILED", "the step left no step", %{})

      assert :ok = Tickets.ended(step)

      assert [comment] = comments(gh)
      assert comment =~ "could not be completed"
      assert comment =~ "the step left no step"
      assert comment =~ "next reply"
    end

    test "a proposal creates each ticket under the ticket, waiting as asked, and says what it made",
         %{gh: gh} do
      StandInGitHub.respond_write(
        gh,
        "POST",
        "repos/#{@repo}/issues",
        %{"number" => 21, "id" => 9021},
        1
      )

      StandInGitHub.respond_write(
        gh,
        "POST",
        "repos/#{@repo}/issues",
        %{"number" => 22, "id" => 9022},
        2
      )

      step =
        ended_step("OUTCOME_COMPLETED", "", %{
          "gap" => %{
            "step" => "proposal",
            "text" => "## Proposed tickets\n\n...",
            "tickets" => [
              %{"title" => "Wait for the button", "parent" => "12", "body" => "Goal: wait."},
              %{
                "title" => "Remove the sleep",
                "parent" => "12",
                "dependsOn" => ["Wait for the button"],
                "body" => "Goal: no sleep."
              }
            ]
          }
        })

      assert :ok = Tickets.ended(step)

      writes = StandInGitHub.writes(gh)

      assert {"POST", "repos/#{@repo}/issues", ["title=Wait for the button", "body=Goal: wait."]} in writes

      assert {"POST", "repos/#{@repo}/issues", ["title=Remove the sleep", "body=Goal: no sleep."]} in writes

      # The ids go as numbers, as the tracker asks for them, and not as text.
      calls = StandInGitHub.gh_calls(gh)
      assert Enum.any?(calls, &String.contains?(&1, "-F sub_issue_id=9021"))
      assert Enum.any?(calls, &String.contains?(&1, "-F issue_id=9021"))

      assert {"POST", @issue <> "/sub_issues", ["sub_issue_id=9021"]} in writes
      assert {"POST", @issue <> "/sub_issues", ["sub_issue_id=9022"]} in writes

      assert {"POST", "repos/#{@repo}/issues/22/dependencies/blocked_by", ["issue_id=9021"]} in writes

      assert [comment] = comments(gh)
      assert comment =~ "#21"
      assert comment =~ "#22"
      assert comment =~ "heph:ready"
      assert comment =~ "Remove the sleep"

      # Nothing is approved: no ticket made is labelled ready, and the ticket's own labels
      # are untouched.
      assert label_writes(gh) == []
      refute Enum.any?(writes, fn {_, endpoint, _} -> endpoint =~ "/labels" end)
    end

    test "a ticket that could not be created is said so, and the rest are still made", %{gh: gh} do
      StandInGitHub.fail_write(gh, "POST", "repos/#{@repo}/issues", "gh: Forbidden (HTTP 403)")

      step =
        ended_step("OUTCOME_COMPLETED", "", %{
          "gap" => %{
            "step" => "proposal",
            "text" => "x",
            "tickets" => [%{"title" => "One", "parent" => "12", "body" => "b"}]
          }
        })

      assert :ok = Tickets.ended(step)

      assert [comment] = comments(gh)
      assert comment =~ "could not be created"
      assert comment =~ "One"
    end

    test "nothing more to ask is commented as written", %{gh: gh} do
      step =
        ended_step("OUTCOME_COMPLETED", "", %{
          "gap" => %{"step" => "nothing", "text" => "## Nothing more to ask\n\nThe gap is X."}
        })

      assert :ok = Tickets.ended(step)

      assert [comment] = comments(gh)
      assert comment =~ "The gap is X."
    end
  end

  describe "after a verdict" do
    defp verdict_for(kind),
      do: %{
        "schema_version" => 0.0,
        "ticket" => 12.0,
        "commit" => "f6f2dc5f9df5fb32ff2a5c36ebee4e3bc9e159d5",
        "verdict" => kind,
        "decides" => "person",
        "decides_because" => "x",
        "epic" => false,
        "requirements" => [],
        "goal" => %{"status" => "not_achieved", "why" => "x"},
        "owed" => [],
        "not_filed" => [],
        "could_not_check" => []
      }

    defp evaluated(kind) do
      {:ok, record} =
        Workloads.create(Map.put(workload(), "completeness", %{"skillDocument" => "x"}))

      {:ok, _} = Events.append(record.id, event(record.id, 1, started()))
      {:ok, _} = Events.append(record.id, event(record.id, 2, finished("OUTCOME_COMPLETED", "")))
      Workloads.keep_record(record.id, %{"completeness" => verdict_for(kind)})
      Workloads.get(record.id)
    end

    test "not met starts a step, saying why" do
      assert :ok = Tickets.ended(evaluated("not_met"))

      assert [step] = steps()
      assert step.workload["gap"]["because"] =~ "not met"
    end

    test "cannot judge starts a step, saying why" do
      assert :ok = Tickets.ended(evaluated("cannot_judge"))

      assert [step] = steps()
      assert step.workload["gap"]["because"] =~ "could not be judged"
    end

    test "met and owed start none" do
      for kind <- ["met", "owed"], do: assert(:ok = Tickets.ended(evaluated(kind)))

      assert steps() == []
    end
  end

  describe "a verdict from before the questioning existed" do
    defp judged(kind, at \\ ~U[2026-11-20 10:00:00Z]) do
      {:ok, record} =
        Workloads.create(Map.put(workload(), "completeness", %{"skillDocument" => "x"}))

      {:ok, _} = Events.append(record.id, event(record.id, 1, started(), at))

      {:ok, _} =
        Events.append(record.id, event(record.id, 2, finished("OUTCOME_COMPLETED", ""), at))

      Workloads.keep_record(record.id, %{"completeness" => verdict_for(kind)})
      Workloads.get(record.id)
    end

    test "that found the goal not met is questioned, saying why" do
      judged("not_met")

      assert [{:started, _}] = Gaps.unquestioned()
      assert [step] = steps()
      assert step.workload["gap"]["because"] =~ "not met"
    end

    test "that could not be judged is questioned" do
      judged("cannot_judge")

      assert [{:started, _}] = Gaps.unquestioned()
    end

    test "that was met or owed is not" do
      judged("met")
      assert Gaps.unquestioned() == []
      judged("owed")
      assert Gaps.unquestioned() == []
    end

    test "is questioned once, however often it is looked for" do
      judged("not_met")

      assert [{:started, id}] = Gaps.unquestioned()
      # It ended after everything before it, as a step that was started later does.
      later = ~U[2026-11-22 00:00:00Z]
      {:ok, _} = Events.append(id, event(id, 1, started(), later))
      {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", ""), later))
      Workloads.keep_record(id, %{"gap" => %{"step" => "nothing", "text" => "x"}})

      assert Gaps.unquestioned() == []
      assert length(steps()) == 1
    end

    test "only the latest verdict counts" do
      judged("not_met", ~U[2026-11-20 10:00:00Z])
      judged("met", ~U[2026-11-20 12:00:00Z])

      assert Gaps.unquestioned() == []
    end

    test "is not looked for in a repository that has not opted in" do
      judged("not_met")
      Application.put_env(:control_plane, :settle, completeness: [], athenaeum_ref: @ref)

      assert Gaps.unquestioned() == []
    end

    test "is looked for by the job that answers replies" do
      judged("not_met")

      assert :ok = perform_job(ControlPlane.Jobs.AnswerReplies, %{})
      assert length(steps()) == 1
    end
  end

  describe "a ticket reopened after Hephaestium closed it" do
    @events "repos/#{@repo}/issues/12/events?per_page=100"

    defp closing_verdict do
      %{
        "schema_version" => 0.0,
        "ticket" => 12.0,
        "commit" => "f6f2dc5f9df5fb32ff2a5c36ebee4e3bc9e159d5",
        "verdict" => "met",
        "decides" => "evidence",
        "decides_because" => "every requirement is settled by evidence",
        "epic" => false,
        "requirements" => [
          %{
            "text" => "r",
            "settled_by" => "evidence",
            "status" => "met",
            "evidence" => %{"kind" => "test", "where" => "w"}
          }
        ],
        "goal" => %{"status" => "achieved", "why" => "x"},
        "owed" => [],
        "not_filed" => [],
        "could_not_check" => []
      }
    end

    # An evaluation that ended at 10:00 with a verdict the evidence decided.
    defp closed_by_heph(verdict \\ closing_verdict()) do
      {:ok, record} =
        Workloads.create(Map.put(workload(), "completeness", %{"skillDocument" => "x"}))

      at = ~U[2026-11-20 10:00:00Z]
      {:ok, _} = Events.append(record.id, event(record.id, 1, started(), at))

      {:ok, _} =
        Events.append(record.id, event(record.id, 2, finished("OUTCOME_COMPLETED", ""), at))

      Workloads.keep_record(record.id, %{"completeness" => verdict})
      Workloads.get(record.id)
    end

    defp reopened(at), do: %{"event" => "reopened", "created_at" => at}

    setup do
      Application.put_env(:control_plane, :settle,
        completeness: [@repo],
        closures: [@repo],
        athenaeum_ref: @ref
      )
    end

    test "is taken as the verdict having been wrong, and starts the questioning", %{gh: gh} do
      closed_by_heph()
      StandInGitHub.respond(gh, @events, [reopened("2026-11-20T12:00:00Z")])

      assert [{:started, _}] = Gaps.reopened()
      assert [step] = steps()
      assert step.workload["gap"]["because"] =~ "reopened"
    end

    test "is not looked for when the ticket was never reopened", %{gh: gh} do
      closed_by_heph()

      StandInGitHub.respond(gh, @events, [
        %{"event" => "closed", "created_at" => "2026-11-20T10:01:00Z"}
      ])

      assert Gaps.reopened() == []
    end

    test "is not counted when it was reopened before the verdict", %{gh: gh} do
      closed_by_heph()
      StandInGitHub.respond(gh, @events, [reopened("2026-11-20T09:00:00Z")])

      assert Gaps.reopened() == []
    end

    test "starts one step, however often it is looked for", %{gh: gh} do
      closed_by_heph()
      StandInGitHub.respond(gh, @events, [reopened("2026-11-20T12:00:00Z")])

      assert [{:started, id}] = Gaps.reopened()
      # It ended after everything before it, as a step that was started later does.
      later = ~U[2026-11-22 00:00:00Z]
      {:ok, _} = Events.append(id, event(id, 1, started(), later))
      {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", ""), later))
      Workloads.keep_record(id, %{"gap" => %{"step" => "nothing", "text" => "x"}})

      assert Gaps.reopened() == []
      assert length(steps()) == 1
    end

    test "is not looked for when the verdict was not one that closes", %{gh: gh} do
      closed_by_heph(Map.put(closing_verdict(), "decides", "person"))
      StandInGitHub.respond(gh, @events, [reopened("2026-11-20T12:00:00Z")])

      assert Gaps.reopened() == []
    end

    test "is not looked for in a repository that does not let Hephaestium close", %{gh: gh} do
      closed_by_heph()
      StandInGitHub.respond(gh, @events, [reopened("2026-11-20T12:00:00Z")])
      Application.put_env(:control_plane, :settle, completeness: [@repo], athenaeum_ref: @ref)

      assert Gaps.reopened() == []
    end

    test "is looked for by the same job that answers replies", %{gh: gh} do
      closed_by_heph()
      StandInGitHub.respond(gh, @events, [reopened("2026-11-20T12:00:00Z")])

      assert :ok = perform_job(ControlPlane.Jobs.AnswerReplies, %{})
      assert length(steps()) == 1
    end
  end

  describe "a person's reply" do
    @comments "repos/#{@repo}/issues/12/comments?per_page=100"

    defp asked(at) do
      ended_step(
        "OUTCOME_COMPLETED",
        "",
        %{"gap" => %{"step" => "question", "text" => "## Next question\n\nWhich?"}},
        at
      )
    end

    defp comment_at(at, body, login \\ "erinversfeldcodes"),
      do: %{"created_at" => at, "body" => body, "user" => %{"login" => login}}

    test "after a question starts the next step", %{gh: gh} do
      asked(~U[2026-11-20 10:00:00Z])
      StandInGitHub.respond(gh, @comments, [comment_at("2026-11-20T11:00:00Z", "The button.")])

      assert [{:started, _}] = Gaps.replies()
      assert Enum.any?(steps(), &(&1.workload["gap"]["because"] =~ "replied"))
    end

    test "is not a comment of Hephaestium's own", %{gh: gh} do
      asked(~U[2026-11-20 10:00:00Z])

      StandInGitHub.respond(gh, @comments, [
        comment_at("2026-11-20T11:00:00Z", "**Hephaestium:** judged whether the goal is met")
      ])

      assert Gaps.replies() == []
    end

    test "is not a comment from before the question", %{gh: gh} do
      asked(~U[2026-11-20 10:00:00Z])
      StandInGitHub.respond(gh, @comments, [comment_at("2026-11-20T09:00:00Z", "Earlier.")])

      assert Gaps.replies() == []
    end

    test "is not waited for when the last step was not a question", %{gh: gh} do
      ended_step("OUTCOME_COMPLETED", "", %{
        "gap" => %{"step" => "nothing", "text" => "## Nothing more to ask\n\nX"}
      })

      StandInGitHub.respond(gh, @comments, [comment_at("2099-01-01T00:00:00Z", "Hello.")])

      assert Gaps.replies() == []
    end

    test "after a step that failed also starts the next", %{gh: gh} do
      ended_step("OUTCOME_FAILED", "the step left no step", %{}, ~U[2026-11-20 10:00:00Z])

      StandInGitHub.respond(gh, @comments, [comment_at("2026-11-20T11:00:00Z", "Try again.")])

      assert [{:started, _}] = Gaps.replies()
    end

    test "is answered once, however often it is looked for", %{gh: gh} do
      asked(~U[2026-11-20 10:00:00Z])
      StandInGitHub.respond(gh, @comments, [comment_at("2026-11-20T11:00:00Z", "The button.")])

      assert [{:started, id}] = Gaps.replies()
      # It ended after everything before it, as a step that was started later does.
      later = ~U[2026-11-22 00:00:00Z]
      {:ok, _} = Events.append(id, event(id, 1, started(), later))
      {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", ""), later))
      Workloads.keep_record(id, %{"gap" => %{"step" => "nothing", "text" => "x"}})

      assert Gaps.replies() == []
    end

    test "is not looked for in a repository that has not opted in", %{gh: gh} do
      asked(~U[2026-11-20 10:00:00Z])
      StandInGitHub.respond(gh, @comments, [comment_at("2026-11-20T11:00:00Z", "The button.")])
      Application.put_env(:control_plane, :settle, completeness: [], athenaeum_ref: @ref)

      assert Gaps.replies() == []
    end

    test "is found by a job" do
      assert :ok = perform_job(ControlPlane.Jobs.AnswerReplies, %{})
    end
  end
end
