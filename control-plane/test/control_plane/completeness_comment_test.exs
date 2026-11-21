defmodule ControlPlane.CompletenessCommentTest do
  use ControlPlane.DataCase

  import ControlPlane.Fixtures

  alias ControlPlane.{CompletenessComment, Events, StandInGitHub, Tickets, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @commit "0123456789abcdef0123456789abcdef01234567"

  # A verdict as the run record holds it: a protobuf Struct, so every number is a float.
  defp verdict(overrides \\ %{}) do
    Map.merge(
      %{
        "schema_version" => 0.0,
        "ticket" => 12.0,
        "commit" => @commit,
        "verdict" => "met",
        "decides" => "evidence",
        "decides_because" => "every requirement is settled by observed evidence",
        "epic" => false,
        "requirements" => [
          %{
            "text" => "The spec waits for hydration",
            "settled_by" => "evidence",
            "status" => "met",
            "evidence" => %{"kind" => "test", "where" => "e2e/tests/account.spec.ts"}
          }
        ],
        "goal" => %{"status" => "achieved", "why" => "the spec no longer clicks early"},
        "owed" => [],
        "not_filed" => [],
        "could_not_check" => []
      },
      overrides
    )
  end

  describe "the comment for a verdict" do
    test "says the verdict, the commit it judged, and that the evidence supports closing" do
      text = CompletenessComment.render(verdict())

      assert text =~ "met"
      assert text =~ "0123456", "the commit, shortened"
      assert text =~ "The evidence supports closing this ticket"
    end

    test "gives each requirement with what settles it, its status and its evidence" do
      text = CompletenessComment.render(verdict())

      assert text =~ "The spec waits for hydration"
      assert text =~ "evidence"
      assert text =~ "test"
      assert text =~ "e2e/tests/account.spec.ts"
    end

    test "says a person decides, and why, when the evidence does not" do
      text =
        CompletenessComment.render(
          verdict(%{"decides" => "person", "decides_because" => "a requirement is wording"})
        )

      assert text =~ "A person decides whether to close this ticket"
      assert text =~ "a requirement is wording"
      refute text =~ "The evidence supports closing"
    end

    test "does not double the full stop when the reason already ends with one" do
      text =
        CompletenessComment.render(
          verdict(%{
            "decides" => "person",
            "decides_because" => "a requirement is wording."
          })
        )

      assert text =~ "a requirement is wording."
      refute text =~ ".."
    end

    test "lists what is owed with its command, its condition and its threshold, and says it stays open" do
      owed = %{
        "what" => "the cache hit rate after the change",
        "run" => "gh cache list",
        "when" => "after ten runs",
        "settled_by" => "no lower than before"
      }

      text =
        CompletenessComment.render(
          verdict(%{
            "verdict" => "owed",
            "decides" => "person",
            "requirements" => [
              %{"text" => "a", "settled_by" => "evidence", "status" => "owed", "gap" => "later"}
            ],
            "goal" => %{"status" => "owed", "why" => "later"},
            "owed" => [owed]
          })
        )

      assert text =~ "the cache hit rate after the change"
      assert text =~ "gh cache list"
      assert text =~ "after ten runs"
      assert text =~ "no lower than before"
      assert text =~ "stays open"
    end

    test "says what is not filed and what could not be checked, only when there is some" do
      bare = CompletenessComment.render(verdict())
      refute bare =~ "Not filed"
      refute bare =~ "Could not check"

      text =
        CompletenessComment.render(
          verdict(%{
            "not_filed" => ["a follow-up with no ticket"],
            "could_not_check" => ["the run logs"]
          })
        )

      assert text =~ "Not filed"
      assert text =~ "a follow-up with no ticket"
      assert text =~ "Could not check"
      assert text =~ "the run logs"
    end

    test "text the agent wrote cannot break the table or add lines" do
      text =
        CompletenessComment.render(
          verdict(%{
            "requirements" => [
              %{
                "text" => "a | b\n\n# a heading",
                "settled_by" => "evidence",
                "status" => "met",
                "evidence" => %{"kind" => "test", "where" => "x"}
              }
            ]
          })
        )

      refute text =~ "\n# a heading"
      assert text =~ "a \\| b"
    end
  end

  describe "the comment for an evaluation that failed" do
    @refused "the verdict does not pass the completeness contract: /requirements/1/evidence/kind: \"structure\" is not one of \"observed\" or \"test\""

    test "names the fault in words and not as a path into the verdict" do
      comment = CompletenessComment.failure(@refused, false)

      assert comment =~ "requirement 2"
      assert comment =~ "\"structure\" is not one of \"observed\" or \"test\""
      refute comment =~ "/requirements/1"
    end

    test "says what a refused verdict means for the ticket, and what happens next" do
      comment = CompletenessComment.failure(@refused, false)

      assert comment =~ "could not be completed"
      assert comment =~ "No judgement was recorded"
      assert comment =~ "judged again"
    end

    test "says a second refusal was already a second attempt" do
      assert CompletenessComment.failure(@refused, true) =~ "second attempt"
      refute CompletenessComment.failure(@refused, false) =~ "second attempt"
    end

    test "gives a failure that is not a refused verdict as it was" do
      comment = CompletenessComment.failure("the agent exited with code 1", false)

      assert comment =~ "could not be completed"
      assert comment =~ "the agent exited with code 1"
      assert comment =~ "No judgement was recorded"
    end

    test "text in the reason cannot add lines or start a block" do
      comment = CompletenessComment.failure("a\n\n# heading\n- item", false)

      refute comment =~ "\n#"
      refute comment =~ "\n-"
    end
  end

  describe "an evaluation that ends" do
    setup do
      dir = Path.join(System.tmp_dir!(), "cp-completeness-#{System.unique_integer([:positive])}")
      stand_in = StandInGitHub.install(dir)
      previous = Application.get_env(:control_plane, :intake)

      Application.put_env(:control_plane, :intake,
        gh: stand_in.gh,
        heph: stand_in.heph,
        repositories: []
      )

      on_exit(fn ->
        Application.put_env(:control_plane, :intake, previous)
        File.rm_rf(dir)
      end)

      evaluation = Map.put(workload(), "completeness", %{"skillDocument" => "the skill"})
      {:ok, record} = Workloads.create(evaluation)
      %{gh: stand_in, id: record.id}
    end

    # An evaluation that is a second attempt, told why the first was refused.
    defp retried(evaluation, refused),
      do: put_in(evaluation, ["completeness"], %{"skillDocument" => "x", "refused" => refused})

    defp ended(id, outcome, reason, extra) do
      {:ok, _} = Events.append(id, event(id, 1, started()))
      {:ok, _} = Events.append(id, event(id, 2, finished(outcome, reason)))
      Workloads.keep_record(id, Map.merge(%{"outcome" => outcome}, extra))
      Tickets.ended(Workloads.get(id))
    end

    defp comments(gh),
      do: for({"POST", @issue <> "/comments", [body]} <- StandInGitHub.writes(gh), do: body)

    defp label_writes(gh),
      do: for({_, @issue <> "/labels" <> _, _} = write <- StandInGitHub.writes(gh), do: write)

    test "starting one changes no label", %{gh: gh, id: id} do
      assert :ok = Tickets.started(Workloads.get(id))
      assert label_writes(gh) == []
    end

    test "a verdict is commented on the ticket, and no label changes", %{gh: gh, id: id} do
      assert :ok = ended(id, "OUTCOME_COMPLETED", "", %{"completeness" => verdict()})

      assert [comment] = comments(gh)
      assert comment =~ "Hephaestium"
      assert comment =~ "The evidence supports closing this ticket"
      assert label_writes(gh) == []
    end

    test "an evaluation that failed says so and why, and changes no label", %{gh: gh, id: id} do
      assert :ok =
               ended(
                 id,
                 "OUTCOME_FAILED",
                 "the verdict is for ticket 13, and the ticket judged is 12",
                 %{}
               )

      assert [comment] = comments(gh)
      assert comment =~ "could not be completed"
      assert comment =~ "the verdict is for ticket 13"
      assert label_writes(gh) == []
    end

    test "a verdict the contract refused is tried once more, told why, with no failure comment",
         %{
           gh: gh,
           id: id
         } do
      reason =
        "the verdict does not pass the completeness contract: /requirements/1/evidence/kind: \"structure\" is not one of \"observed\" or \"test\""

      Application.put_env(:control_plane, :settle,
        completeness: [@repo],
        athenaeum_ref: "v0.26.0"
      )

      StandInGitHub.respond(gh, @issue, %{"number" => 12, "state" => "open"})

      StandInGitHub.respond_text(
        gh,
        "repos/erinversfeldcodes/athenaeum/contents/skills/ticket-completeness/SKILL.md?ref=v0.26.0",
        "the skill"
      )

      StandInGitHub.heph_prints(gh, workload())

      assert :ok = ended(id, "OUTCOME_FAILED", reason, %{})

      assert comments(gh) == [], "a retry follows, so the first failure is not announced"

      assert [retry] =
               Enum.filter(
                 Workloads.for_ticket(@repo, 12),
                 &(&1.id != id and is_map(&1.workload["completeness"]))
               )

      assert retry.state == "queued"
      assert retry.workload["completeness"]["refused"] == reason
    end

    test "a retry that is refused too is announced as failed, and not tried again", %{
      gh: gh,
      id: id
    } do
      # The first attempt has ended, so nothing but the rule can stop another.
      {:ok, _} = Events.append(id, event(id, 1, started()))
      {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))

      Application.put_env(:control_plane, :settle,
        completeness: [@repo],
        athenaeum_ref: "v0.26.0"
      )

      # Everything a retry would need is there, so that only the rule stops it.
      StandInGitHub.respond(gh, @issue, %{"number" => 12, "state" => "open"})

      StandInGitHub.respond_text(
        gh,
        "repos/erinversfeldcodes/athenaeum/contents/skills/ticket-completeness/SKILL.md?ref=v0.26.0",
        "the skill"
      )

      StandInGitHub.heph_prints(gh, workload())

      {:ok, retry} = Workloads.create(retried(workload(), "the first refusal"))

      reason = "the verdict does not pass the completeness contract: a second refusal"
      assert :ok = ended(retry.id, "OUTCOME_FAILED", reason, %{})

      assert [comment] = comments(gh)
      assert comment =~ "could not be completed"
      assert comment =~ "a second refusal"

      assert Enum.count(Workloads.for_ticket(@repo, 12), &is_map(&1.workload["completeness"])) ==
               2
    end

    test "a failure that is not a refused verdict is announced and not retried", %{
      gh: gh,
      id: id
    } do
      Application.put_env(:control_plane, :settle,
        completeness: [@repo],
        athenaeum_ref: "v0.26.0"
      )

      assert :ok = ended(id, "OUTCOME_FAILED", "the agent exited with code 1", %{})

      assert [comment] = comments(gh)
      assert comment =~ "the agent exited with code 1"

      assert Enum.count(Workloads.for_ticket(@repo, 12), &is_map(&1.workload["completeness"])) ==
               1
    end

    test "a refusal that cannot be retried is announced", %{gh: gh, id: id} do
      Application.put_env(:control_plane, :settle, completeness: [], athenaeum_ref: "v0.26.0")
      reason = "the verdict does not pass the completeness contract: x"

      assert :ok = ended(id, "OUTCOME_FAILED", reason, %{})

      assert [comment] = comments(gh)
      assert comment =~ "could not be completed"
    end

    test "a completed evaluation with no verdict in its record says so", %{gh: gh, id: id} do
      assert :ok = ended(id, "OUTCOME_COMPLETED", "", %{})

      assert [comment] = comments(gh)
      assert comment =~ "left no verdict"
    end
  end
end
