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

    test "a completed evaluation with no verdict in its record says so", %{gh: gh, id: id} do
      assert :ok = ended(id, "OUTCOME_COMPLETED", "", %{})

      assert [comment] = comments(gh)
      assert comment =~ "left no verdict"
    end
  end
end
