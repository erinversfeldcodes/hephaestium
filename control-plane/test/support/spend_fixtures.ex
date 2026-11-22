defmodule ControlPlane.SpendFixtures do
  @moduledoc """
  Workloads that spent something, kept so that the ledger has their rows: for the tests of what
  the ledger is read for.
  """

  import ControlPlane.Fixtures

  alias ControlPlane.Workloads

  @usd %{"unit" => "COST_UNIT_USD", "source" => "SOURCE_REPORTED_BY_RUNTIME"}
  @tokens %{"unit" => "COST_UNIT_TOKENS", "source" => "SOURCE_REPORTED_BY_RUNTIME"}

  def usd(amount), do: Map.put(@usd, "amount", amount)
  def tokens(amount), do: Map.put(@tokens, "amount", amount)

  @doc "A workload of this ticket that spent these costs, kept so that the ledger has its rows."
  def spent(ticket, costs, opts \\ []) do
    {:ok, created} =
      workload()
      |> put_in(["ticket", "number"], Integer.to_string(ticket))
      |> maybe_put(["ticket", "parent"], opts[:epic] && Integer.to_string(opts[:epic]))
      |> maybe_put_repository(opts[:repository])
      |> maybe_put(["review"], opts[:review])
      |> maybe_put(["revision"], opts[:revision])
      |> Workloads.create()

    record =
      %{
        "outcome" => "OUTCOME_COMPLETED",
        "finishedAt" => opts[:at] || "2026-11-22T07:00:00Z",
        "costs" => costs
      }
      |> maybe_put(
        ["pullRequest"],
        opts[:branch] && %{"number" => "52", "headBranch" => opts[:branch]}
      )
      |> maybe_put(["steps"], opts[:steps])

    :ok = Workloads.keep_record(created.id, record)
    created
  end

  defp maybe_put(map, _path, nil), do: map
  defp maybe_put(map, [key], value), do: Map.put(map, key, value)
  defp maybe_put(map, path, value), do: put_in(map, path, value)

  defp maybe_put_repository(map, nil), do: map

  defp maybe_put_repository(map, "owner/name") do
    map |> put_in(["repository", "owner"], "someone") |> put_in(["repository", "name"], "else")
  end
end
