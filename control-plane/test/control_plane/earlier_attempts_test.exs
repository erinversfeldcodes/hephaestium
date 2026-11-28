defmodule ControlPlane.EarlierAttemptsTest do
  @moduledoc """
  What a new implementation is told of why the ticket's earlier attempts ended. The records
  are the contracts' own valid run records (`cases/runtime/RunRecord/valid/`), read from the
  files; the only additions are a gate's last lines and a step list, which those cases leave
  out and which `run_record.proto` defines (`GateRun.output`, `Step`).
  """

  use ControlPlane.DataCase

  import Ecto.Query
  import ControlPlane.Fixtures

  alias ControlPlane.{EarlierAttempts, Repo, Workload, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @ticket 12
  @cases Path.expand("../../../cases/runtime/RunRecord/valid", __DIR__)

  defp contract(name), do: Path.join(@cases, name) |> File.read!() |> Jason.decode!()

  # An attempt on the ticket that ended in the state, the given minutes ago.
  defp attempt(state, record, minutes_ago \\ 0, reason \\ nil) do
    {:ok, created} = Workloads.create(workload())
    at = DateTime.add(DateTime.utc_now(), -minutes_ago * 60, :second)

    Repo.update_all(from(w in Workload, where: w.id == ^created.id),
      set: [state: state, record: record, reason: reason, inserted_at: at]
    )

    created.id
  end

  defp attempts(request \\ %{"id" => "new"}),
    do: EarlierAttempts.add(request, @repo, @ticket)["earlierAttempts"]

  test "a first attempt's request is unchanged" do
    assert EarlierAttempts.add(%{"id" => "new"}, @repo, @ticket) == %{"id" => "new"}
  end

  test "a completed attempt contributes nothing" do
    attempt("completed", %{"outcome" => "OUTCOME_COMPLETED"})
    refute Map.has_key?(EarlierAttempts.add(%{}, @repo, @ticket), "earlierAttempts")
  end

  test "after a failed gate the request carries the gate and the end of its output" do
    record = contract("failed-with-a-failed-gate.json")
    [gate] = record["gates"]
    gate = Map.merge(gate, %{"output" => "line one\n1) test boom failed", "omittedLines" => 3})
    attempt("failed", %{record | "gates" => [gate]}, 0, record["reason"])

    assert [%{"ended" => ended, "why" => why}] = attempts()
    assert ended =~ "gate"
    assert why =~ "test"
    assert why =~ "1) test boom failed"
  end

  test "after a timeout the request says it timed out and in which step" do
    record =
      Map.put(contract("timed-out.json"), "steps", [
        %{"kind" => "KIND_IMPLEMENTATION"},
        %{"kind" => "KIND_REVIEW", "lens" => "rust", "round" => 1}
      ])

    attempt("timed_out", record, 0, record["reason"])

    assert [%{"ended" => ended, "why" => why}] = attempts()
    assert ended =~ "timed out"
    assert ended =~ "review"
    assert ended =~ "rust"
    assert why =~ "Stopped after 1800 seconds"
  end

  test "only the latest three that failed are carried, newest first" do
    for n <- 1..4, do: attempt("failed", %{}, 10 - n, "reason #{n}")
    attempt("completed", %{}, 0)

    assert ["reason 4", "reason 3", "reason 2"] =
             Enum.map(attempts(), &String.trim(&1["why"]))
  end

  test "what is carried is at most 8,000 bytes and says what was cut" do
    for n <- 1..3, do: attempt("failed", %{}, n, String.duplicate("é", 20_000))

    carried = attempts()
    assert length(carried) == 3
    assert carried |> Enum.map(&byte_size(&1["ended"] <> &1["why"])) |> Enum.sum() <= 8_000
    assert Enum.all?(carried, &String.valid?(&1["why"]))
    assert Enum.all?(carried, &(&1["why"] =~ "cut"))
  end
end
