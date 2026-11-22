defmodule ControlPlane.Ledger do
  @moduledoc """
  What workloads spent, kept as an append-only ledger (ADR 0036): one row for each
  cost-bearing step of every workload, written once when the workload's record is kept, with
  the ticket, epic, pull request and branch it concerned.

  A step is an entry of the record's `steps` (the implementation, each reading and each
  revision of a run that read its own change, ADR 0035); a record that lists none has one
  step, 0, made of its totals. A unit is kept as it was said and never converted into
  another, and a source stays on its row. A record kept twice counts once, because a row is
  identified by its workload, step, unit and source. The record is the source of truth: a
  ledger that could not be written is repaired by the backfill.
  """

  import Ecto.Query

  require Logger

  alias ControlPlane.{Repo, SpendEntry, Workload}

  @doc """
  Write the rows of a workload's record. Returns how many rows were written, which is none
  for a record that spent nothing, or that has already been written. A cost that cannot be
  read is skipped and logged, and does not stop the others.
  """
  @spec record(%Workload{}) :: non_neg_integer()
  def record(%Workload{record: record} = workload) when is_map(record) do
    now = DateTime.utc_now()

    case rows(workload, now) do
      [] ->
        0

      rows ->
        {written, _} =
          Repo.insert_all(SpendEntry, rows,
            on_conflict: :nothing,
            conflict_target: [:workload_id, :step, :unit, :source]
          )

        written
    end
  end

  def record(_workload), do: 0

  @doc """
  Write the rows of every workload that has a record and no rows: those kept before the
  ledger, and any whose write failed. Returns how many workloads were written.
  """
  @spec backfill() :: non_neg_integer()
  def backfill do
    unwritten =
      Repo.all(
        from w in Workload,
          as: :workload,
          where: not is_nil(w.record),
          where:
            not exists(
              from e in SpendEntry, where: e.workload_id == parent_as(:workload).id, select: 1
            ),
          order_by: w.inserted_at
      )

    Enum.count(unwritten, fn workload -> record(workload) > 0 end)
  end

  defp rows(workload, now) do
    record = workload.record

    steps =
      case record["steps"] do
        [_ | _] = steps ->
          steps
          |> Enum.with_index()
          |> Enum.map(fn {step, at} ->
            {at, step_kind(step["kind"]), step["lens"], step["round"] || 0, step["costs"]}
          end)

        _ ->
          [
            {0, workload_kind(workload.workload), review_lens(workload.workload), 0,
             record["costs"]}
          ]
      end

    context = context(workload, now)

    for {at, kind, lens, round, costs} <- steps,
        cost <- List.wrap(costs),
        row = row(workload, context, {at, kind, lens, round}, cost),
        do: row
  end

  defp context(workload, now) do
    record = workload.record
    request = workload.workload || %{}
    pull = record["pullRequest"] || %{}
    revision = request["revision"] || %{}
    review = request["review"] || %{}

    %{
      epic_number: positive(integer(get_in(request, ["ticket", "parent"]))),
      pull_request_number:
        integer(pull["number"]) || integer(revision["pullRequestNumber"]) ||
          integer(review["pullRequestNumber"]),
      branch: pull["headBranch"] || revision["branch"],
      occurred_at: microseconds(time(record["finishedAt"]) || workload.finished_at || now),
      recorded_at: now
    }
  end

  defp row(workload, context, {at, kind, lens, round}, cost) do
    with unit when is_binary(unit) <- cost["unit"],
         amount when is_number(amount) <- cost["amount"] do
      parts = parts(cost["breakdown"])

      %{
        id: Ecto.UUID.generate(),
        workload_id: workload.id,
        step: at,
        kind: kind,
        lens: lens,
        round: round,
        unit: name(unit, "COST_UNIT_"),
        source: name(cost["source"] || "SOURCE_UNSPECIFIED", "SOURCE_") |> source_name(),
        amount: amount * 1.0,
        input: parts[:input],
        output: parts[:output],
        cache_creation: parts[:cache_creation],
        cache_read: parts[:cache_read],
        repository: workload.repository,
        ticket_number: workload.ticket_number
      }
      |> Map.merge(context)
    else
      _ ->
        Logger.warning(
          "a cost of workload #{workload.id} could not be read, so it is not in the ledger: " <>
            inspect(cost)
        )

        nil
    end
  end

  # The parts of a token amount, only when the runtime gave them.
  defp parts(%{} = breakdown) do
    %{
      input: number(breakdown["input"]),
      output: number(breakdown["output"]),
      cache_creation: number(breakdown["cacheCreation"]),
      cache_read: number(breakdown["cacheRead"])
    }
  end

  defp parts(_), do: %{}

  defp step_kind("KIND_" <> kind), do: String.downcase(kind)
  defp step_kind(_), do: "implementation"

  # What a workload that lists no steps was: a review, an evaluation of the ticket, a step of
  # the questioning, a revision, or a first run.
  defp workload_kind(%{"review" => _}), do: "review"
  defp workload_kind(%{"completeness" => _}), do: "completeness"
  defp workload_kind(%{"gap" => _}), do: "gap"
  defp workload_kind(%{"revision" => _}), do: "revision"
  defp workload_kind(_), do: "implementation"

  defp review_lens(%{"review" => %{"lens" => lens}}) when is_binary(lens), do: lens
  defp review_lens(_), do: nil

  defp name(text, prefix) when is_binary(text),
    do: text |> String.replace_prefix(prefix, "") |> String.downcase()

  defp name(_, _), do: "unspecified"

  defp source_name("reported_by_runtime"), do: "reported"
  defp source_name("estimated_by_hephaestium"), do: "estimated"
  defp source_name(other), do: other

  defp number(value) when is_number(value), do: value * 1.0
  defp number(_), do: nil

  # A 64-bit number in the Protobuf JSON mapping is a string.
  defp integer(value) when is_integer(value), do: value

  defp integer(value) when is_binary(value) do
    case Integer.parse(value) do
      {number, ""} -> number
      _ -> nil
    end
  end

  defp integer(_), do: nil

  defp positive(number) when is_integer(number) and number > 0, do: number
  defp positive(_), do: nil

  # The column holds microseconds, and a time written to the second has none.
  defp microseconds(%DateTime{microsecond: {value, _}} = time),
    do: %{time | microsecond: {value, 6}}

  defp time(text) when is_binary(text) do
    case DateTime.from_iso8601(text) do
      {:ok, time, _} -> time
      _ -> nil
    end
  end

  defp time(_), do: nil
end
