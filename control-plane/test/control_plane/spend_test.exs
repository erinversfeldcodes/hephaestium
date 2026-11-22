defmodule ControlPlane.SpendTest do
  @moduledoc """
  What a ticket, an epic and a branch have spent, and what was spent each day, are queries
  over the ledger (ADR 0036). Each unit is on its own: dollars and tokens are never added to
  each other, and nothing converts one to the other.
  """

  use ControlPlane.DataCase

  import ControlPlane.SpendFixtures

  alias ControlPlane.Spend

  @repo "erinversfeldcodes/gazebolang"

  describe "a ticket" do
    test "has what all its workloads spent, each unit on its own, and how many there were" do
      spent(12, [usd(0.5), tokens(1_000.0)])

      spent(12, [usd(0.25), tokens(500.0)],
        revision: %{"pullRequestNumber" => "52", "branch" => "heph/x", "attempt" => 1}
      )

      spent(13, [usd(9.0)])

      ticket = Spend.ticket(@repo, 12)

      assert_in_delta ticket.totals["usd"], 0.75, 1.0e-9
      assert_in_delta ticket.totals["tokens"], 1_500.0, 1.0e-9
      assert ticket.workloads == 2
    end

    test "says what each kind of step spent" do
      spent(12, [usd(0.9)],
        steps: [
          %{"kind" => "KIND_IMPLEMENTATION", "costs" => [usd(0.5)]},
          %{"kind" => "KIND_REVIEW", "lens" => "rust", "round" => 1, "costs" => [usd(0.3)]},
          %{"kind" => "KIND_REVISION", "round" => 1, "costs" => [usd(0.1)]}
        ]
      )

      spent(12, [usd(0.2)],
        review: %{"pullRequestNumber" => "52", "lens" => "spec", "lensDocument" => "x"}
      )

      by_kind = Spend.ticket(@repo, 12).by_kind

      assert_in_delta by_kind["implementation"]["usd"], 0.5, 1.0e-9

      assert_in_delta by_kind["review"]["usd"],
                      0.5,
                      1.0e-9,
                      "reading before the push and a review after it"

      assert_in_delta by_kind["revision"]["usd"], 0.1, 1.0e-9
    end

    test "never adds tokens to dollars, and has nothing for a unit nobody spent" do
      spent(12, [tokens(1_000.0)])

      ticket = Spend.ticket(@repo, 12)

      assert ticket.totals == %{"tokens" => 1_000.0}
      refute Map.has_key?(ticket.totals, "usd")
    end

    test "has the parts of its tokens where the runtime gave them, and says how much they cover" do
      with_parts =
        Map.put(tokens(1_000.0), "breakdown", %{
          "input" => 10.0,
          "output" => 20.0,
          "cacheCreation" => 30.0,
          "cacheRead" => 940.0
        })

      spent(12, [with_parts])
      spent(12, [tokens(500.0)])

      parts = Spend.ticket(@repo, 12).parts

      assert {parts.input, parts.output, parts.cache_creation, parts.cache_read} ==
               {10.0, 20.0, 30.0, 940.0}

      assert parts.covers == 1_000.0, "only the tokens whose parts are known"
    end

    test "has no parts when no runtime gave any, and nothing at all when nothing was spent" do
      spent(12, [tokens(500.0)])
      assert Spend.ticket(@repo, 12).parts == nil

      empty = Spend.ticket(@repo, 99)
      assert empty.totals == %{}
      assert empty.workloads == 0
    end

    test "is only of its own repository" do
      spent(12, [usd(0.5)])
      spent(12, [usd(7.0)], repository: "owner/name")

      assert_in_delta Spend.ticket(@repo, 12).totals["usd"], 0.5, 1.0e-9
      assert_in_delta Spend.ticket("someone/else", 12).totals["usd"], 7.0, 1.0e-9
    end
  end

  describe "an epic" do
    test "is the sum over the tickets under it, each ticket's own beside it, most spent first" do
      spent(12, [usd(0.5)], epic: 7)
      spent(12, [usd(0.25)], epic: 7)
      spent(13, [usd(2.0)], epic: 7)
      spent(14, [usd(100.0)], epic: 8)
      spent(15, [usd(50.0)])

      epic = Spend.epic(@repo, 7)

      assert_in_delta epic.totals["usd"], 2.75, 1.0e-9
      assert epic.workloads == 3

      assert Enum.map(epic.tickets, & &1.ticket) == [13, 12]
      assert_in_delta hd(epic.tickets).totals["usd"], 2.0, 1.0e-9
      assert_in_delta List.last(epic.tickets).totals["usd"], 0.75, 1.0e-9
    end

    test "keeps the spend a ticket made under the epic it had then, whatever it has now" do
      spent(12, [usd(1.0)], epic: 7)
      spent(12, [usd(3.0)], epic: 8)

      assert_in_delta Spend.epic(@repo, 7).totals["usd"], 1.0, 1.0e-9
      assert_in_delta Spend.epic(@repo, 8).totals["usd"], 3.0, 1.0e-9
    end

    test "has nothing when no ticket is under it" do
      assert Spend.epic(@repo, 404).totals == %{}
      assert Spend.epic(@repo, 404).tickets == []
    end
  end

  describe "a branch" do
    test "is what the workloads that concerned it spent" do
      spent(12, [usd(0.5)], branch: "heph/12-a-fix")
      spent(12, [usd(0.25)], branch: "heph/12-a-fix")
      spent(13, [usd(4.0)], branch: "heph/13-other")

      branch = Spend.branch(@repo, "heph/12-a-fix")

      assert_in_delta branch.totals["usd"], 0.75, 1.0e-9
      assert branch.workloads == 2
    end
  end

  describe "by day" do
    test "groups what was spent by the UTC day it ended, newest first, each unit on its own" do
      now = ~U[2026-11-22 12:00:00Z]
      spent(12, [usd(0.5), tokens(100.0)], at: "2026-11-22T07:00:00Z")
      spent(12, [usd(0.25)], at: "2026-11-22T23:30:00Z")
      spent(13, [usd(2.0)], at: "2026-11-20T01:00:00Z")
      spent(14, [usd(50.0)], at: "2026-10-01T00:00:00Z")

      days = Spend.by_day(@repo, 5, now)

      assert Enum.map(days, & &1.day) == [~D[2026-11-22], ~D[2026-11-20]]
      assert_in_delta hd(days).totals["usd"], 0.75, 1.0e-9
      assert_in_delta hd(days).totals["tokens"], 100.0, 1.0e-9
      assert_in_delta List.last(days).totals["usd"], 2.0, 1.0e-9
    end
  end

  describe "the report a person reads" do
    test "of a ticket: its totals, and each kind of step on a line of its own" do
      spent(12, [usd(0.9), tokens(2_000.0)],
        steps: [
          %{"kind" => "KIND_IMPLEMENTATION", "costs" => [usd(0.5), tokens(1_500.0)]},
          %{
            "kind" => "KIND_REVIEW",
            "lens" => "rust",
            "round" => 1,
            "costs" => [usd(0.4), tokens(500.0)]
          }
        ]
      )

      report = Spend.report(@repo, {:ticket, 12})

      assert report =~ "Ticket #12 in #{@repo}: $0.900, 2000 tokens across 1 workload"
      assert report =~ ~r/the implementation\s+\$0\.500 and 1500 tokens/
      assert report =~ ~r/reviewing\s+\$0\.400 and 500 tokens/
    end

    test "of an epic: its total, and each ticket on a line, most spent first" do
      spent(12, [usd(0.5)], epic: 7)
      spent(13, [usd(2.0)], epic: 7)

      report = Spend.report(@repo, {:epic, 7})

      assert report =~ "Epic #7 in #{@repo}: $2.500 across 2 tickets"
      assert Regex.run(~r/#13.*\n.*#12/s, report), report
    end

    test "of a branch, and of the days" do
      spent(12, [usd(0.5)], branch: "heph/12-a-fix", at: "2026-11-22T07:00:00Z")

      assert Spend.report(@repo, {:branch, "heph/12-a-fix"}) =~
               "Branch heph/12-a-fix in #{@repo}: $0.500 across 1 workload"

      day = Spend.report(@repo, {:days, 5}, ~U[2026-11-22 12:00:00Z])
      assert day =~ "2026-11-22"
      assert day =~ "$0.500"
    end

    test "says plainly when nothing was spent" do
      assert Spend.report(@repo, {:ticket, 404}) =~ "Nothing has been spent on ticket #404"
      assert Spend.report(@repo, {:epic, 404}) =~ "Nothing has been spent under epic #404"
    end
  end
end
