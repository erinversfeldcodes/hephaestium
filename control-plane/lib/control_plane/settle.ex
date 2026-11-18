defmodule ControlPlane.Settle do
  @moduledoc """
  Done means green: a ticket whose workload completed is not done until its pull
  request's required checks are green.

  For each completed workload with a pull request whose ticket is not yet settled,
  this reads the pull request, the checks the base branch's ruleset requires, and
  the check runs on the pull request's head commit, with `gh`:

    * merged: done. Closed without merging: abandoned.
    * any required check ended as anything but success, skipped or neutral:
      failed, naming it. A failure is not waited on while others still run.
    * every required check ended well: done.
    * anything else (a check still running or not yet seen, no checks at all, or
      GitHub unable to say) waits, and nothing is written: unknown is never green.

  Only the newest run of a check counts, because a cancelled duplicate run leaves a
  failed run of the same name behind. The ticket is marked settled only once its
  labels and comment were written; if they could not be, it is tried again.
  """

  alias ControlPlane.{GitHub, Tickets, Workloads}

  @good ["success", "skipped", "neutral"]

  @doc "Look at every workload awaiting settlement. Returns what became of each."
  @spec run() :: [%{workload_id: String.t(), result: term()}]
  def run do
    for workload <- Workloads.awaiting_settlement() do
      %{workload_id: workload.id, result: settle(workload)}
    end
  end

  defp settle(workload) do
    with {:ok, number} <- pull_number(workload),
         {:ok, pull} <- GitHub.get("repos/#{workload.repository}/pulls/#{number}"),
         {:ok, verdict} <- verdict(workload, pull),
         :ok <- write(workload, verdict) do
      Workloads.settle(workload.id)
      verdict
    else
      {:error, reason} -> {:waiting, to_string(reason)}
      {:waiting, reason} -> {:waiting, reason}
    end
  end

  defp write(workload, verdict) do
    case Tickets.settled(workload, verdict) do
      :ok -> :ok
      {:error, message} -> {:error, "could not write to the ticket: #{message}"}
    end
  end

  defp pull_number(workload) do
    url = get_in(workload.record, ["pullRequest", "url"]) || ""

    case Regex.run(~r{^https://github\.com/([^/]+/[^/]+)/pull/(\d+)$}, url) do
      [_, repository, number] when repository == workload.repository -> {:ok, number}
      _ -> {:error, "the pull request #{inspect(url)} is not one of #{workload.repository}"}
    end
  end

  defp verdict(_workload, %{"merged" => true}), do: {:ok, :done}
  defp verdict(_workload, %{"state" => "closed"}), do: {:ok, :abandoned}

  defp verdict(workload, %{"head" => %{"sha" => sha}, "base" => %{"ref" => base}}) do
    with {:ok, rules} <- GitHub.get("repos/#{workload.repository}/rules/branches/#{base}"),
         {:ok, runs} <-
           GitHub.get("repos/#{workload.repository}/commits/#{sha}/check-runs?per_page=100") do
      judge(required(rules), newest(runs["check_runs"] || []))
    end
  end

  defp verdict(_workload, _pull), do: {:error, "the pull request has no head and base"}

  defp required(rules) do
    for %{
          "type" => "required_status_checks",
          "parameters" => %{"required_status_checks" => checks}
        } <-
          rules,
        %{"context" => context} <- checks,
        uniq: true,
        do: context
  end

  # The newest run of each name.
  defp newest(runs) do
    runs
    |> Enum.group_by(& &1["name"])
    |> Map.new(fn {name, rs} -> {name, Enum.max_by(rs, & &1["id"])} end)
  end

  defp judge(required, runs) do
    names = if required == [], do: Map.keys(runs), else: required
    looked_at = Enum.map(names, &{&1, runs[&1]})

    failed =
      for {name, run} <- looked_at,
          run != nil and run["status"] == "completed" and run["conclusion"] not in @good,
          do: name

    unfinished =
      for {name, run} <- looked_at, is_nil(run) or run["status"] != "completed", do: name

    cond do
      failed != [] ->
        {:ok, {:failed, Enum.sort(failed)}}

      names == [] ->
        {:waiting, "no checks have been reported yet"}

      unfinished != [] ->
        {:waiting, "still waiting for: #{Enum.join(Enum.sort(unfinished), ", ")}"}

      true ->
        {:ok, :done}
    end
  end
end
