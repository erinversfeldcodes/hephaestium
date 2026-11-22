defmodule ControlPlane.LedgerTest do
  @moduledoc """
  Spend is kept as an append-only ledger (ADR 0036): one row for each cost-bearing step of
  every workload, written once when the workload's record is kept, with the ticket, epic, pull
  request and branch it concerned. Each unit is on its own row and nothing converts one to
  another.
  """

  use ControlPlane.DataCase

  import Ecto.Query
  import ExUnit.CaptureLog
  import ControlPlane.Fixtures

  alias ControlPlane.{Ledger, Repo, SpendEntry, Workload, Workloads}

  @usd %{"unit" => "COST_UNIT_USD", "source" => "SOURCE_REPORTED_BY_RUNTIME"}
  @tokens %{"unit" => "COST_UNIT_TOKENS", "source" => "SOURCE_REPORTED_BY_RUNTIME"}
  @parts %{"input" => 10.0, "output" => 20.0, "cacheCreation" => 30.0, "cacheRead" => 940.0}

  defp usd(amount), do: Map.put(@usd, "amount", amount)
  defp tokens(amount), do: Map.put(@tokens, "amount", amount)
  defp tokens(amount, parts), do: amount |> tokens() |> Map.put("breakdown", parts)

  # A workload kept with this record, and any changes to what it was built from.
  defp kept(record, edit \\ & &1) do
    {:ok, created} = Workloads.create(edit.(workload()))
    :ok = Workloads.keep_record(created.id, record)
    created
  end

  defp entries(id),
    do: Repo.all(from(e in SpendEntry, where: e.workload_id == ^id, order_by: [e.step, e.unit]))

  defp record(extra),
    do:
      Map.merge(
        %{
          "outcome" => "OUTCOME_COMPLETED",
          "finishedAt" => "2026-11-22T07:00:00Z",
          "pullRequest" => %{
            "url" => "https://github.com/erinversfeldcodes/gazebolang/pull/52",
            "number" => "52",
            "headBranch" => "heph/12-a-fix"
          }
        },
        extra
      )

  defp with_parent(number),
    do: fn workload -> put_in(workload, ["ticket", "parent"], Integer.to_string(number)) end

  describe "a record that lists its steps" do
    setup do
      record =
        record(%{
          "costs" => [usd(0.9), tokens(1_000.0, @parts)],
          "steps" => [
            %{
              "kind" => "KIND_IMPLEMENTATION",
              "costs" => [
                usd(0.5),
                tokens(600.0, %{
                  "input" => 5.0,
                  "output" => 15.0,
                  "cacheCreation" => 20.0,
                  "cacheRead" => 560.0
                })
              ]
            },
            %{
              "kind" => "KIND_REVIEW",
              "lens" => "rust",
              "round" => 1,
              "verdict" => "approved",
              "costs" => [
                usd(0.4),
                tokens(400.0, %{
                  "input" => 5.0,
                  "output" => 5.0,
                  "cacheCreation" => 10.0,
                  "cacheRead" => 380.0
                })
              ]
            }
          ]
        })

      %{created: kept(record, with_parent(7))}
    end

    test "is one row for each step and unit, with who and what it concerned", %{created: created} do
      rows = entries(created.id)

      assert Enum.map(rows, &{&1.step, &1.kind, &1.unit}) == [
               {0, "implementation", "tokens"},
               {0, "implementation", "usd"},
               {1, "review", "tokens"},
               {1, "review", "usd"}
             ]

      review = Enum.find(rows, &(&1.step == 1 and &1.unit == "usd"))
      assert review.lens == "rust"
      assert review.round == 1
      assert review.amount == 0.4
      assert review.source == "reported"
      assert review.repository == "erinversfeldcodes/gazebolang"
      assert review.ticket_number == 12
      assert review.epic_number == 7
      assert review.pull_request_number == 52
      assert review.branch == "heph/12-a-fix"
      assert review.occurred_at == ~U[2026-11-22 07:00:00.000000Z]
    end

    test "carries the parts of a token cost on that row and none on a dollar row", %{
      created: created
    } do
      rows = entries(created.id)

      tokens = Enum.find(rows, &(&1.step == 0 and &1.unit == "tokens"))

      assert {tokens.input, tokens.output, tokens.cache_creation, tokens.cache_read} ==
               {5.0, 15.0, 20.0, 560.0}

      dollars = Enum.find(rows, &(&1.step == 0 and &1.unit == "usd"))

      assert {dollars.input, dollars.output, dollars.cache_creation, dollars.cache_read} ==
               {nil, nil, nil, nil}
    end

    test "keeping the same record again writes nothing more", %{created: created} do
      before = length(entries(created.id))

      :ok = Workloads.keep_record(created.id, Workloads.get(created.id).record)

      assert length(entries(created.id)) == before
    end

    test "the rows of the steps add up to the run's totals, each unit on its own", %{
      created: created
    } do
      by_unit =
        entries(created.id)
        |> Enum.group_by(& &1.unit, & &1.amount)
        |> Map.new(fn {unit, amounts} -> {unit, Enum.sum(amounts)} end)

      assert_in_delta by_unit["usd"], 0.9, 1.0e-9
      assert_in_delta by_unit["tokens"], 1_000.0, 1.0e-9
    end
  end

  describe "a record that lists no steps" do
    test "is one row for each total, at step 0, as the kind of workload it was" do
      record = record(%{"costs" => [usd(0.3), tokens(200.0)]})

      plain = kept(record)

      assert Enum.map(entries(plain.id), &{&1.step, &1.kind, &1.unit}) == [
               {0, "implementation", "tokens"},
               {0, "implementation", "usd"}
             ]

      revision =
        kept(
          record,
          &Map.put(&1, "revision", %{
            "pullRequestNumber" => "52",
            "branch" => "heph/12-an-earlier-title",
            "attempt" => 1
          })
        )

      assert Enum.map(entries(revision.id), & &1.kind) == ["revision", "revision"]

      review =
        kept(
          record,
          &Map.put(&1, "review", %{
            "pullRequestNumber" => "52",
            "lens" => "rust",
            "lensDocument" => "x"
          })
        )

      assert Enum.map(entries(review.id), & &1.kind) == ["review", "review"]
      assert Enum.map(entries(review.id), & &1.lens) == ["rust", "rust"]

      completeness = kept(record, &Map.put(&1, "completeness", %{"skill" => "x"}))
      assert Enum.map(entries(completeness.id), & &1.kind) == ["completeness", "completeness"]

      gap = kept(record, &Map.put(&1, "gap", %{"skill" => "x", "because" => "y"}))
      assert Enum.map(entries(gap.id), & &1.kind) == ["gap", "gap"]
    end

    test "a revision's pull request and branch are the ones it was made for, when its record names none" do
      revision =
        kept(
          %{"outcome" => "OUTCOME_COMPLETED", "costs" => [usd(0.3)]},
          &Map.put(&1, "revision", %{
            "pullRequestNumber" => "52",
            "branch" => "heph/12-an-earlier-title",
            "attempt" => 1
          })
        )

      [row] = entries(revision.id)

      assert row.pull_request_number == 52
      assert row.branch == "heph/12-an-earlier-title"
    end

    test "a token cost that gave no parts has none on its row" do
      created = kept(record(%{"costs" => [tokens(200.0)]}))

      [row] = entries(created.id)

      assert {row.input, row.output, row.cache_creation, row.cache_read} == {nil, nil, nil, nil}
    end
  end

  describe "what is and is not written" do
    test "a record with no costs writes nothing, and a ticket with no parent has no epic" do
      created = kept(record(%{}))
      assert entries(created.id) == []

      with_cost = kept(record(%{"costs" => [usd(0.1)]}))
      [row] = entries(with_cost.id)
      assert row.epic_number == nil
    end

    test "a cost that cannot be read is skipped and logged, and the rest and the record are kept" do
      level = Logger.level()
      Logger.configure(level: :warning)
      on_exit(fn -> Logger.configure(level: level) end)

      {created, log} =
        with_log(fn ->
          kept(record(%{"costs" => [%{"unit" => "COST_UNIT_USD", "amount" => "lots"}, usd(0.2)]}))
        end)

      assert [row] = entries(created.id)
      assert row.amount == 0.2
      assert log =~ "could not be read"
      assert Workloads.get(created.id).record["costs"] |> length() == 2, "the record is whole"
    end

    test "a unit this ledger does not know is kept as it was said, never converted" do
      created =
        kept(
          record(%{
            "costs" => [
              %{
                "unit" => "COST_UNIT_CREDITS",
                "amount" => 3.0,
                "source" => "SOURCE_ESTIMATED_BY_HEPHAESTIUM"
              }
            ]
          })
        )

      [row] = entries(created.id)

      assert {row.unit, row.source, row.amount} == {"credits", "estimated", 3.0}
    end
  end

  describe "the backfill" do
    test "writes a stored record once, and a second pass writes nothing" do
      {:ok, created} = Workloads.create(workload())

      stored =
        record(%{"costs" => [usd(0.3), tokens(200.0)]})

      # Stored the way a record was before the ledger: nothing wrote its rows.
      Repo.update_all(from(w in Workload, where: w.id == ^created.id), set: [record: stored])
      assert entries(created.id) == []

      assert Ledger.backfill() == 1
      assert length(entries(created.id)) == 2

      assert Ledger.backfill() == 0
      assert length(entries(created.id)) == 2
    end

    test "leaves alone a workload that has no record or no costs" do
      {:ok, _} = Workloads.create(workload())
      kept(record(%{}))

      assert Ledger.backfill() == 0
    end
  end

  describe "the ledger and its workloads" do
    test "rows are kept when the workload they came from is removed, and do not stop it being" do
      created = kept(record(%{"costs" => [usd(0.3)]}))
      assert [_] = entries(created.id)

      Repo.delete_all(from(w in Workload, where: w.id == ^created.id))

      assert [%{amount: 0.3}] = entries(created.id)
    end
  end

  describe "the ledger only grows" do
    test "an update or a delete of a row is refused by the database itself" do
      created = kept(record(%{"costs" => [usd(0.3)]}))
      assert [_] = entries(created.id)

      assert_raise Postgrex.Error, ~r/append-only/, fn ->
        Repo.query!("update spend_entries set amount = 0", [], mode: :savepoint)
      end

      assert_raise Postgrex.Error, ~r/append-only/, fn ->
        Repo.query!("delete from spend_entries", [], mode: :savepoint)
      end

      assert [%{amount: 0.3}] = entries(created.id)
    end
  end
end
