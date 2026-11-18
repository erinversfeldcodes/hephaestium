defmodule ControlPlane.Settle do
  @moduledoc """
  Done means green: a ticket whose workload completed is not done until every check
  on its pull request is green, whether or not the ruleset requires it (ADR 0024).

  For each completed workload with a pull request whose ticket is not yet settled,
  this reads the pull request, the checks the base branch's ruleset requires, and
  the check runs on the pull request's head commit, with `gh`:

    * merged: done. Closed without merging: abandoned.
    * any check ended as anything but success, skipped or neutral: failed, naming
      it. A failure is not waited on while others still run.
    * every check ended well, and every required one has appeared: done.
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
         {:ok, verdict} <- verdict(workload, pull, number),
         :ok <- act(workload, verdict) do
      outcome(workload, verdict)
    else
      {:error, reason} -> {:waiting, to_string(reason)}
      {:waiting, reason} -> {:waiting, reason}
    end
  end

  defp outcome(_workload, {:rerun, _} = verdict), do: {:waiting, rerun_message(verdict)}
  defp outcome(workload, {:revise, _, _, _}), do: {:revising, attempt(workload) + 1}
  defp outcome(workload, verdict), do: finish(workload, verdict)

  defp finish(workload, verdict) do
    Workloads.settle(workload.id)
    verdict
  end

  defp rerun_message({:rerun, runs}),
    do: "re-running cancelled: #{runs |> Enum.map(&elem(&1, 0)) |> Enum.join(", ")}"

  # A verdict that is a rerun asks GitHub to re-run each cancelled job and counts the
  # ones it accepted; any other verdict is written to the ticket.
  defp act(workload, {:rerun, runs}) do
    results =
      for {name, id} <- runs do
        case GitHub.write("POST", "repos/#{workload.repository}/actions/jobs/#{id}/rerun", []) do
          {:ok, _} ->
            Workloads.count_rerun(workload.id, name)
            :ok

          {:error, message} ->
            {:error, "could not re-run #{name}: #{message}"}
        end
      end

    Enum.find(results, :ok, &match?({:error, _}, &1))
  end

  # A revision: the stored workload again, on the pull request's head, told which
  # checks failed and what they said. It is kept, queued and the first workload
  # handed on in one step; the ticket is told afterwards, and a comment that
  # cannot be written does not undo the revision.
  defp act(workload, {:revise, runs, pull, number}) do
    attempt = attempt(workload) + 1

    failures =
      for {name, run} <- runs do
        %{"name" => name, "result" => run["conclusion"], "excerpt" => excerpt(workload, run)}
      end

    revision =
      workload.workload
      |> Map.put("id", Ecto.UUID.generate())
      |> put_in(["repository", "baseCommit"], pull["head"]["sha"])
      |> Map.put("revision", %{
        "pullRequestNumber" => to_string(number),
        "pullRequestUrl" => "https://github.com/#{workload.repository}/pull/#{number}",
        "branch" => pull["head"]["ref"],
        "attempt" => attempt,
        "failures" => failures
      })

    case Workloads.hand_on(workload, revision) do
      {:ok, _child} ->
        Tickets.revising(workload, Enum.map(failures, & &1["name"]), attempt, revisions())
        :ok

      {:error, reason} ->
        {:error, "could not start a revision: #{inspect(reason)}"}
    end
  end

  defp act(workload, verdict), do: write(workload, verdict)

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

  defp verdict(_workload, %{"merged" => true}, _number), do: {:ok, :done}
  defp verdict(_workload, %{"state" => "closed"}, _number), do: {:ok, :abandoned}

  defp verdict(workload, %{"head" => %{"sha" => sha}, "base" => %{"ref" => base}} = pull, number) do
    with {:ok, rules} <- GitHub.get("repos/#{workload.repository}/rules/branches/#{base}"),
         {:ok, runs} <-
           GitHub.get("repos/#{workload.repository}/commits/#{sha}/check-runs?per_page=100") do
      judge(workload, required(rules), newest(runs["check_runs"] || []), {pull, number})
    end
  end

  defp verdict(_workload, _pull, _number), do: {:error, "the pull request has no head and base"}

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

  # What a failing check said, for the agent: the check's own title and summary and the
  # tail of its job's log, cut to what fits. A log that cannot be read is no reason
  # not to revise, so the revision gets what there is, or says there was nothing.
  @excerpt_bytes 8_000
  @separator "\n--- the end of the job's log ---\n"

  defp excerpt(workload, run) do
    output = run["output"] || %{}

    summary =
      [output["title"], output["summary"]]
      |> Enum.filter(&(is_binary(&1) and &1 != ""))
      |> Enum.join("\n")
      |> cut_start(1_500)

    log =
      case GitHub.text("repos/#{workload.repository}/actions/jobs/#{run["id"]}/logs") do
        {:ok, text} ->
          text
          |> strip_colours()
          |> tail(@excerpt_bytes - byte_size(summary) - byte_size(@separator))

        {:error, _} ->
          ""
      end

    cond do
      summary != "" and log != "" -> summary <> @separator <> log
      summary != "" -> summary
      log != "" -> log
      true -> "(no output was available for this check)"
    end
  end

  defp cut_start(text, bytes) when byte_size(text) <= bytes, do: text

  defp cut_start(text, bytes),
    do: binary_part(text, 0, bytes) |> String.replace(~r/[^[:print:]\n]\z/u, "")

  # The last `bytes` of the text, from the start of a line.
  defp tail(text, bytes) when byte_size(text) <= bytes, do: String.trim(text)

  defp tail(text, bytes) do
    chunk = binary_part(text, byte_size(text) - bytes, bytes)

    case :binary.split(chunk, "\n") do
      [_partial, rest] -> String.trim(rest)
      [whole] -> String.trim(whole)
    end
  end

  defp strip_colours(text), do: String.replace(text, ~r/\e\[[0-9;]*[A-Za-z]/, "")

  defp attempt(workload), do: get_in(workload.workload, ["revision", "attempt"]) || 0

  defp judge(workload, required, runs, pull_and_number) do
    # Every check on the commit counts, not only the ones the ruleset requires: a
    # failing check is investigated whether or not it blocks a merge (ADR 0024). The
    # required ones must also have appeared.
    names = Enum.uniq(required ++ Map.keys(runs))
    looked_at = Enum.map(names, &{&1, runs[&1]})

    ended =
      for {name, run} <- looked_at, run != nil and run["status"] == "completed", do: {name, run}

    bad = for {name, run} <- ended, run["conclusion"] not in @good, do: {name, run}
    cancelled = for {name, run} <- bad, run["conclusion"] == "cancelled", do: {name, run}
    failed = Enum.map(bad, &elem(&1, 0))

    unfinished =
      for {name, run} <- looked_at, is_nil(run) or run["status"] != "completed", do: name

    real = bad -- cancelled
    revisable = revisable(workload, pull_and_number)

    cond do
      failed != [] and bad == cancelled and rerunnable?(workload, cancelled) ->
        {:ok, {:rerun, for({name, run} <- cancelled, do: {name, run["id"]})}}

      real != [] and revisable == :yes ->
        {pull, number} = pull_and_number
        {:ok, {:revise, real, pull, number}}

      failed != [] ->
        {:ok, failed(workload, Enum.sort(failed), real != [], revisable)}

      names == [] ->
        {:waiting, "no checks have been reported yet"}

      unfinished != [] ->
        {:waiting, "still waiting for: #{Enum.join(Enum.sort(unfinished), ", ")}"}

      true ->
        {:ok, :done}
    end
  end

  # Every cancelled check has re-runs left. With none allowed (the default) none has.
  defp rerunnable?(workload, cancelled) do
    max = reruns()
    done = workload.reruns || %{}

    max > 0 and Enum.all?(cancelled, fn {name, _} -> Map.get(done, name, 0) < max end)
  end

  # Whether a failing pull request is revised: revisions are on, there are some left,
  # and the pull request is a branch of this repository that the runtime made. Anything
  # else says why not.
  defp revisable(workload, {pull, _number}) do
    head = pull["head"] || %{}
    ref = head["ref"] || ""

    cond do
      revisions() == 0 ->
        :off

      attempt(workload) >= revisions() ->
        :exhausted

      not String.starts_with?(ref, "heph/") ->
        {:no, "its branch #{inspect(ref)} is not one the runtime made (heph/...)"}

      get_in(head, ["repo", "full_name"]) != workload.repository ->
        {:no, "its branch is in another repository"}

      true ->
        :yes
    end
  end

  # The verdict that ends in failure. When revisions were on and the failure was a
  # real one, it says why there was no revision, or what the revisions did.
  defp failed(_workload, names, false, _revisable), do: {:failed, names}
  defp failed(_workload, names, true, :off), do: {:failed, names}

  defp failed(_workload, names, true, {:no, reason}),
    do: {:failed, names, "It was not revised: #{reason}."}

  defp failed(workload, names, true, :exhausted) do
    lines =
      for {w, index} <- Enum.with_index(Workloads.chain(workload)), index > 0 do
        what = if w.reason in [nil, ""], do: w.state, else: "#{w.state}: #{w.reason}"
        "- revision #{index}: #{what}"
      end

    {:failed, names,
     "Revisions made: #{attempt(workload)} of #{revisions()}.\n" <> Enum.join(lines, "\n")}
  end

  defp revisions,
    do: setting(:revisions)

  defp reruns, do: setting(:reruns)

  # A setting of settling, zero (none) unless set.
  defp setting(name),
    do: (Application.get_env(:control_plane, :settle) || []) |> Keyword.get(name, 0)
end
