defmodule ControlPlane.Spend do
  @moduledoc """
  What a ticket, an epic and a branch have spent, and what was spent each day: roll-ups over
  the spend ledger (ADR 0036). Each unit is on its own, so a total is a map from a unit to an
  amount, dollars and tokens are never added to each other, and nothing converts one to the
  other. A unit nobody spent in is not in the map.

  An epic counts the spend of a ticket under the epic it had when the workload was built, so
  its total does not change when a ticket is moved.
  """

  import Ecto.Query

  alias ControlPlane.{Amounts, Repo, SpendEntry}

  @doc """
  What a ticket has spent: its totals, what each kind of step spent, how many workloads spent
  it, and the parts of its tokens where the runtime gave them (`nil` when none did), with how
  many tokens those parts cover.
  """
  @spec ticket(String.t(), integer()) :: map()
  def ticket(repository, number) do
    rows =
      rows(
        from(e in SpendEntry, where: e.repository == ^repository and e.ticket_number == ^number)
      )

    %{
      totals: totals(rows),
      by_kind:
        rows |> Enum.group_by(& &1.kind) |> Map.new(fn {kind, rows} -> {kind, totals(rows)} end),
      workloads: workloads(rows),
      parts: parts(rows),
      steps: steps(repository, number)
    }
  end

  @doc """
  What each step of a ticket spent, in the order the steps ran (the workloads oldest first,
  then each workload's steps). A step carries its workload, kind, lens and round, its dollars
  and tokens, the tokens by kind, and whether its workload is still running, in which case it
  is what has been spent so far.
  """
  @spec steps(String.t(), integer()) :: [map()]
  def steps(repository, number) do
    Repo.all(
      from e in SpendEntry,
        join: w in ControlPlane.Workload,
        on: w.id == e.workload_id,
        where: e.repository == ^repository and e.ticket_number == ^number,
        order_by: [w.inserted_at, w.id, e.step, e.unit, e.source],
        select: {e, w.state}
    )
    |> Enum.chunk_by(fn {e, _} -> {e.workload_id, e.step} end)
    |> Enum.map(fn [{first, state} | _] = group ->
      %{
        workload_id: first.workload_id,
        step: first.step,
        kind: first.kind,
        lens: first.lens,
        round: first.round
      }
      |> Map.merge(figures(Enum.map(group, &elem(&1, 0)), state))
    end)
  end

  @doc """
  What one workload has spent, from its ledger rows: dollars, tokens and the tokens by kind,
  all zero when nothing was spent. Dollars are not split by kind of token: the ledger holds no
  such figure.
  """
  @spec figures([%SpendEntry{}], String.t()) :: map()
  def figures(rows, state) do
    tokens = Enum.filter(rows, &(&1.unit == "tokens"))
    sum = fn rows, field -> rows |> Enum.map(&(Map.get(&1, field) || 0.0)) |> Enum.sum() end

    %{
      usd: sum.(Enum.filter(rows, &(&1.unit == "usd")), :amount),
      tokens: sum.(tokens, :amount),
      tokens_by_kind: %{
        input: sum.(tokens, :input),
        output: sum.(tokens, :output),
        cache_creation: sum.(tokens, :cache_creation),
        cache_read: sum.(tokens, :cache_read)
      },
      running: state not in ControlPlane.Workloads.terminal_states()
    }
  end

  @doc "What each of a ticket's workloads has spent, by workload id, for those that spent."
  @spec by_workload(String.t(), integer()) :: %{String.t() => map()}
  def by_workload(repository, number) do
    Repo.all(
      from e in SpendEntry,
        join: w in ControlPlane.Workload,
        on: w.id == e.workload_id,
        where: e.repository == ^repository and e.ticket_number == ^number,
        select: {e, w.state}
    )
    |> Enum.group_by(fn {e, _} -> e.workload_id end)
    |> Map.new(fn {id, [{_, state} | _] = group} ->
      {id, figures(Enum.map(group, &elem(&1, 0)), state)}
    end)
  end

  @doc """
  What an epic has spent over the tickets under it, with each ticket's own, most spent first
  (dollars, then tokens, then the lower ticket number).
  """
  @spec epic(String.t(), integer()) :: map()
  def epic(repository, epic) do
    rows =
      rows(from(e in SpendEntry, where: e.repository == ^repository and e.epic_number == ^epic))

    tickets =
      rows
      |> Enum.group_by(& &1.ticket_number)
      |> Enum.map(fn {ticket, rows} ->
        %{ticket: ticket, totals: totals(rows), workloads: workloads(rows)}
      end)
      |> Enum.sort_by(
        &{-Map.get(&1.totals, "usd", 0.0), -Map.get(&1.totals, "tokens", 0.0), &1.ticket}
      )

    %{totals: totals(rows), workloads: workloads(rows), tickets: tickets}
  end

  @doc "What the workloads that concerned a branch spent."
  @spec branch(String.t(), String.t()) :: map()
  def branch(repository, branch) do
    rows = rows(from(e in SpendEntry, where: e.repository == ^repository and e.branch == ^branch))
    %{totals: totals(rows), workloads: workloads(rows)}
  end

  @doc """
  What a repository spent on each UTC day over the last `days`, newest day first. A day is the
  one the workload ended.
  """
  @spec by_day(String.t(), pos_integer(), DateTime.t()) :: [%{day: Date.t(), totals: map()}]
  def by_day(repository, days, now \\ DateTime.utc_now()) do
    since = DateTime.add(now, -days * 86_400, :second)

    rows =
      rows(from(e in SpendEntry, where: e.repository == ^repository and e.occurred_at >= ^since))

    rows
    |> Enum.group_by(&DateTime.to_date(&1.occurred_at))
    |> Enum.map(fn {day, rows} -> %{day: day, totals: totals(rows)} end)
    |> Enum.sort_by(& &1.day, {:desc, Date})
  end

  defp rows(query), do: Repo.all(query)

  # Each unit on its own.
  defp totals(rows) do
    rows
    |> Enum.group_by(& &1.unit, & &1.amount)
    |> Map.new(fn {unit, amounts} -> {unit, Enum.sum(amounts)} end)
  end

  defp workloads(rows), do: rows |> Enum.map(& &1.workload_id) |> Enum.uniq() |> length()

  # The parts of the tokens whose runtime gave them, and how many tokens that covers.
  defp parts(rows) do
    with_parts =
      Enum.filter(rows, &(&1.unit == "tokens" and not is_nil(&1.input) and not is_nil(&1.output)))

    case with_parts do
      [] ->
        nil

      rows ->
        sum = fn field -> rows |> Enum.map(&(Map.get(&1, field) || 0.0)) |> Enum.sum() end

        %{
          input: sum.(:input),
          output: sum.(:output),
          cache_creation: sum.(:cache_creation),
          cache_read: sum.(:cache_read),
          covers: rows |> Enum.map(& &1.amount) |> Enum.sum()
        }
    end
  end

  # The kinds of step a ledger row can have, with how a person is told of each.
  @kinds [
    {"implementation", "the implementation"},
    {"review", "reviewing"},
    {"revision", "fixing"},
    {"completeness", "judging"},
    {"gap", "questioning"}
  ]

  @doc "The kinds of step, in the order they are told, with the words for each."
  @spec kinds() :: [{String.t(), String.t()}]
  def kinds, do: @kinds

  @doc """
  What a person reads: the roll-up of a ticket, an epic or a branch, or the last days, as
  text. `what` is `{:ticket, number}`, `{:epic, number}`, `{:branch, name}` or `{:days, n}`.
  """
  @spec report(String.t(), tuple(), DateTime.t()) :: String.t()
  def report(repository, what, now \\ DateTime.utc_now())

  def report(repository, {:ticket, number}, _now) do
    ticket = ticket(repository, number)

    if ticket.totals == %{} do
      "Nothing has been spent on ticket ##{number} in #{repository}."
    else
      lines =
        for {kind, label} <- @kinds, totals = ticket.by_kind[kind], totals != %{} do
          "  #{String.pad_trailing(label, 24)}#{Amounts.describe(totals, " and ")}"
        end

      ["Ticket ##{number} in #{repository}: #{summary(ticket)}" | lines] |> Enum.join("\n")
    end
  end

  def report(repository, {:epic, number}, _now) do
    epic = epic(repository, number)

    if epic.totals == %{} do
      "Nothing has been spent under epic ##{number} in #{repository}."
    else
      lines =
        for ticket <- epic.tickets do
          "  ##{String.pad_trailing(Integer.to_string(ticket.ticket), 8)}#{Amounts.describe(ticket.totals)} (#{count(ticket.workloads, "workload")})"
        end

      [
        "Epic ##{number} in #{repository}: #{Amounts.describe(epic.totals)} across #{count(length(epic.tickets), "ticket")}"
        | lines
      ]
      |> Enum.join("\n")
    end
  end

  def report(repository, {:branch, branch}, _now) do
    rollup = branch(repository, branch)

    if rollup.totals == %{} do
      "Nothing has been spent on branch #{branch} in #{repository}."
    else
      "Branch #{branch} in #{repository}: #{summary(rollup)}"
    end
  end

  def report(repository, {:days, days}, now) do
    case by_day(repository, days, now) do
      [] ->
        "Nothing has been spent in #{repository} in the last #{days} days."

      rows ->
        lines =
          for %{day: day, totals: totals} <- rows, do: "  #{day}  #{Amounts.describe(totals)}"

        ["Spent in #{repository} over the last #{days} days:" | lines] |> Enum.join("\n")
    end
  end

  defp summary(%{totals: totals, workloads: workloads}),
    do: "#{Amounts.describe(totals)} across #{count(workloads, "workload")}"

  defp count(1, noun), do: "1 #{noun}"
  defp count(number, noun), do: "#{number} #{noun}s"
end
