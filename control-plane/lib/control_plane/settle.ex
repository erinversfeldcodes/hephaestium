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
    * every check ended well, and every required one has appeared: done. So has every
      check a gate was left to (ADR 0028): the workspace could not run the gate, the
      repository says that check runs the same work, and the ticket is not done on
      the strength of a gate nobody ran.
    * anything else (a check still running or not yet seen, no checks at all, or
      GitHub unable to say) waits, and nothing is written: unknown is never green.

  Only the newest run of a check counts, because a cancelled duplicate run leaves a
  failed run of the same name behind. The ticket is marked settled only once its
  labels and comment were written; if they could not be, it is tried again.
  """

  require Logger

  alias ControlPlane.{
    Account,
    Evaluations,
    Finding,
    GitHub,
    ReviewApproval,
    Reviews,
    Tickets,
    Workloads
  }

  @good ["success", "skipped", "neutral"]

  @doc "Look at every workload awaiting settlement. Returns what became of each."
  @spec run() :: [%{workload_id: String.t(), result: term()}]
  def run do
    for workload <- Workloads.awaiting_settlement() do
      %{workload_id: workload.id, result: settle_safely(workload)}
    end
  end

  # One workload that cannot be settled is that workload's result and nothing more: the
  # others behind it are still settled, and the job does not crash for every ticket
  # because of one. The failure is logged, and the workload is looked at again next time.
  defp settle_safely(workload) do
    settle(workload)
  rescue
    exception ->
      message = "could not settle #{workload.id}: " <> Exception.message(exception)
      Logger.error(message)
      {:error, message}
  end

  @doc """
  Look again at the pull requests that were settled while they waited for a person, and whose
  branch has since fallen behind its base. A pull request is settled when its checks are green,
  and then it waits for a review or a merge while the base branch moves on; settled, nothing
  looked at it again, so it went stale unseen. One that is open, behind, still green and has a
  revision left, and whose base gained something, is made awaiting settlement again, so the next
  pass brings the base in as it would have before it was settled. A pull request that is not
  green is left, because looking again would announce its failure again.
  """
  @spec stale() :: [%{workload_id: String.t(), result: :unsettled}]
  def stale do
    since = DateTime.add(DateTime.utc_now(), -14 * 86_400, :second)

    for workload <- Workloads.settled_pull_requests(since), stale?(workload) do
      Workloads.unsettle(workload.id)
      %{workload_id: workload.id, result: :unsettled}
    end
  end

  defp stale?(workload) do
    with {:ok, number} <- Workloads.pull_request_number(workload),
         {:ok, %{"state" => "open", "mergeable_state" => "behind"} = pull} <-
           GitHub.get("repos/#{workload.repository}/pulls/#{number}"),
         {:ok, :done} <- verdict(workload, pull, number),
         :yes <- revisable(workload, {pull, number}),
         {:ok, %{"commits" => [_ | _]}} <- base_moved(workload, pull, number) do
      true
    else
      _ -> false
    end
  rescue
    exception ->
      Logger.error(
        "could not look for a stale pull request of #{workload.id}: " <>
          Exception.message(exception)
      )

      false
  end

  defp settle(workload) do
    with {:ok, number} <- Workloads.pull_request_number(workload),
         {:ok, pull} <- GitHub.get("repos/#{workload.repository}/pulls/#{number}"),
         {:ok, verdict} <- verdict(workload, pull, number),
         {:ok, verdict} <- behind(workload, pull, number, verdict),
         {:ok, verdict, notes} <- reviews(workload, pull, number, verdict),
         {:ok, verdict} <- merge(workload, pull, number, verdict),
         notes = with_left_to_ci(workload, pull, verdict, notes),
         :ok <- act(workload, verdict, notes),
         :ok <- evaluate(workload, pull, verdict) do
      outcome(workload, verdict)
    else
      {:error, reason} -> {:waiting, to_string(reason)}
      {:waiting, reason} -> {:waiting, reason}
    end
  end

  # A pull request that has merged, for a repository that opted in, is followed by a
  # judgement of whether its ticket's goal is met, if the ticket is still open. It is made
  # once, because the ticket is settled once, and it never undoes the settling: a judgement
  # that could not be started is logged, and the ticket is not settled again for it.
  defp evaluate(workload, pull, verdict) do
    if merged?(pull, verdict) do
      case Evaluations.after_merge(workload) do
        {:started, id} ->
          Logger.info("started evaluation #{id} of ticket #{workload.ticket_number}")

        {:skipped, reason} ->
          Logger.info("no evaluation of ticket #{workload.ticket_number}: #{inspect(reason)}")
      end
    end

    :ok
  rescue
    exception ->
      Logger.error(
        "could not start an evaluation of ticket #{workload.ticket_number}: " <>
          Exception.message(exception)
      )

      :ok
  end

  defp merged?(%{"merged" => true}, _verdict), do: true
  defp merged?(_pull, {:done, :merged}), do: true
  defp merged?(_pull, _verdict), do: false

  defp outcome(_workload, {:rerun, _} = verdict), do: {:waiting, rerun_message(verdict)}
  defp outcome(workload, {:revise, _, _, _}), do: {:revising, attempt(workload) + 1}
  defp outcome(workload, {:revise_review, _, _, _}), do: {:revising, attempt(workload) + 1}
  defp outcome(workload, {:revise_base, _, _, _}), do: {:revising, attempt(workload) + 1}

  defp outcome(workload, {:failed, names, extra, :review}),
    do: finish(workload, {:failed, names, extra})

  defp outcome(workload, verdict), do: finish(workload, verdict)

  # A pull request whose checks are all green but whose branch is behind its base is
  # brought up to date by a revision that is told what came in, before it is reviewed,
  # because the reviews would be of a head that is about to change (ADR 0031). Only a
  # branch the runtime made, in this repository, and only while revisions are left:
  # anything else is settled as it always was. A conflict is not handled here: the
  # executor ends such a run naming the files, and a revision would only spend a run.
  defp behind(workload, %{"state" => "open", "mergeable_state" => "behind"} = pull, number, :done) do
    case revisable(workload, {pull, number}) do
      :yes ->
        case base_moved(workload, pull, number) do
          {:ok, %{"commits" => [_ | _]} = moved} -> {:ok, {:revise_base, moved, pull, number}}
          {:ok, _nothing_came_in} -> {:ok, :done}
          {:error, reason} -> {:error, reason}
        end

      _ ->
        {:ok, :done}
    end
  end

  defp behind(_workload, _pull, _number, verdict), do: {:ok, verdict}

  # What the base gained since the pull request's head, newest first and as short as the
  # agent needs, and which of the files it changed the pull request changes too.
  @moved_commits 50

  defp base_moved(workload, pull, number) do
    sha = get_in(pull, ["head", "sha"])
    base = get_in(pull, ["base", "ref"]) || "main"

    with {:ok, compare} <- GitHub.get("repos/#{workload.repository}/compare/#{sha}...#{base}"),
         {:ok, mine} <-
           GitHub.get("repos/#{workload.repository}/pulls/#{number}/files?per_page=100") do
      commits =
        (compare["commits"] || [])
        |> Enum.reverse()
        |> Enum.take(@moved_commits)
        |> Enum.map(&commit_line/1)

      theirs = for file <- compare["files"] || [], do: file["filename"]
      ours = MapSet.new(for file <- List.wrap(mine), do: file["filename"])

      {:ok, %{"commits" => commits, "overlappingFiles" => Enum.filter(theirs, &(&1 in ours))}}
    end
  end

  defp commit_line(%{"sha" => sha, "commit" => %{"message" => message}}) do
    subject = message |> String.split("\n", parts: 2) |> hd() |> String.slice(0, 200)
    "#{String.slice(sha, 0, 7)} #{subject}"
  end

  # A pull request whose checks are all green is reviewed, for the repositories that
  # opted in, and what the verdicts say decides what becomes of it (ADR 0026). If the
  # reviews cannot be asked for, the ticket waits: it is not done on the strength of
  # checks alone when a review was meant to happen.
  defp reviews(workload, %{"state" => "open"} = pull, number, :done) do
    if Reviews.enabled?(workload.repository) and pull["merged"] != true do
      with {:ok, asked} <- Reviews.request(workload, pull, number) do
        sha = get_in(pull, ["head", "sha"])
        reviewed(workload, pull, number, Reviews.assess(workload, number, sha, asked))
      else
        {:error, message} -> {:waiting, "could not ask for reviews: #{message}"}
      end
    else
      {:ok, :done, nil}
    end
  end

  defp reviews(_workload, _pull, _number, verdict), do: {:ok, verdict, nil}

  # Every lens that blocks approved this commit. Where a review App is configured the
  # pull request is approved on GitHub too, so GitHub's own rule is met, and this pass
  # waits: the pull request it read says it is blocked until the approval is counted, and
  # the next pass reads it again before anything is merged.
  defp reviewed(workload, pull, number, {:approved, lines}) do
    notes = "Reviews:\n" <> Enum.join(lines, "\n")

    case ReviewApproval.ensure(workload.repository, number, get_in(pull, ["head", "sha"]), lines) do
      :disabled -> {:ok, :done, notes}
      :approved -> {:ok, :done, notes}
      :submitted -> {:waiting, "approved on GitHub by the review App; waiting for it to count"}
      {:error, message} -> {:waiting, "could not approve on GitHub: #{message}"}
    end
  end

  defp reviewed(_workload, _pull, _number, {:pending, lenses}),
    do: {:waiting, "waiting for reviews: #{Enum.join(lenses, ", ")}"}

  defp reviewed(_workload, _pull, _number, {:blocked, reasons}) do
    lenses = Enum.map(reasons, &elem(&1, 0))
    lines = for {lens, why} <- reasons, do: "- #{lens}: #{why}. That is no approval."
    {:ok, {:failed, lenses, Enum.join(lines, "\n"), :review}, nil}
  end

  defp reviewed(workload, pull, number, {:revise, findings}) do
    case revisable(workload, {pull, number}) do
      :yes ->
        {:ok, {:revise_review, findings, pull, number}, nil}

      why ->
        lenses = findings |> Enum.map(& &1.lens) |> Enum.uniq()

        {:ok, {:failed, lenses, not_revised(workload, why) <> "\n\n" <> found(findings), :review},
         nil}
    end
  end

  defp found(findings) do
    Enum.map_join(findings, "\n", fn %{lens: lens, finding: finding} ->
      "- #{lens} [#{finding["severity"]}] #{finding["title"]}: #{finding["evidence"]}" <>
        if(finding["revision"], do: " Revise: #{finding["revision"]}", else: "")
    end)
  end

  defp not_revised(_workload, :off), do: "It was not revised: revisions are off."
  defp not_revised(_workload, {:no, reason}), do: "It was not revised: #{reason}."

  defp not_revised(workload, :exhausted) do
    "It was not revised again: revisions made #{used(workload)} of #{revisions()}."
  end

  # Merging a pull request whose every check is green, for the repositories that
  # opted in (`HEPH_AUTOMERGE`). It is never a bypass: GitHub's own account of the pull
  # request has to be `clean`, which it is only when the repository's rules are met
  # (a required review, a branch up to date), because an account that may bypass
  # those rules would otherwise merge a blocked pull request through the API without
  # anyone saying so. Anything else is reported on the ticket and left to a person.
  defp merge(workload, %{"state" => "open"} = pull, number, :done) when pull != %{} do
    if automerge?(workload.repository) and pull["merged"] != true do
      merge_open(workload, pull, number)
    else
      {:ok, :done}
    end
  end

  defp merge(_workload, _pull, _number, verdict), do: {:ok, verdict}

  defp merge_open(workload, pull, number) do
    head = pull["head"] || %{}
    ref = head["ref"] || ""

    cond do
      pull["mergeable_state"] in [nil, "unknown"] ->
        {:waiting, "GitHub has not worked out whether the pull request can be merged"}

      not String.starts_with?(ref, "heph/") ->
        not_merged("it is not a pull request the runtime made (heph/...)")

      get_in(head, ["repo", "full_name"]) != workload.repository ->
        not_merged("its branch is in another repository")

      pull["mergeable_state"] != "clean" ->
        not_merged(unmergeable(pull["mergeable_state"]))

      true ->
        put_merge(workload, number, head["sha"])
    end
  end

  defp put_merge(workload, number, sha) do
    case GitHub.write("PUT", "repos/#{workload.repository}/pulls/#{number}/merge", [
           "merge_method=squash",
           "sha=#{sha}"
         ]) do
      {:ok, _} -> {:ok, {:done, :merged}}
      {:error, message} -> merge_refused(message)
    end
  end

  # GitHub saying the pull request cannot be merged (not allowed, conflicting, the head
  # moved) is an answer, to be reported once. Anything else may pass, so it waits.
  defp merge_refused(message) do
    if Regex.match?(~r/HTTP (405|409|422)/, message),
      do: not_merged("GitHub refused the merge: #{message}"),
      else: {:waiting, "could not merge: #{message}"}
  end

  defp not_merged(reason), do: {:ok, {:done, {:not_merged, reason}}}

  defp unmergeable("blocked"),
    do: "it is blocked: its rules are not met (for example a required review)"

  defp unmergeable("behind"), do: "its branch is behind the base"
  defp unmergeable("dirty"), do: "it has conflicts with the base"
  defp unmergeable("draft"), do: "it is a draft"
  defp unmergeable(state), do: "GitHub reports it as #{state}"

  defp finish(workload, verdict) do
    Workloads.settle(workload.id)
    verdict
  end

  defp rerun_message({:rerun, runs}),
    do: "re-running cancelled: #{runs |> Enum.map(&elem(&1, 0)) |> Enum.join(", ")}"

  # A verdict that is a rerun asks GitHub to re-run each cancelled job and counts the
  # ones it accepted; any other verdict is written to the ticket.
  defp act(workload, {:rerun, runs}, _notes) do
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

  # A revision: the stored workload again, on the pull request's head, told what is
  # wrong: the checks that failed and what they said, or the blocking findings of the
  # reviews. It is kept, queued and the first workload handed on in one step; the
  # ticket is told afterwards, and a comment that cannot be written does not undo the
  # revision.
  defp act(workload, {:revise, runs, pull, number}, _notes) do
    failures =
      for {name, run} <- runs do
        %{"name" => name, "result" => run["conclusion"], "excerpt" => excerpt(workload, run)}
      end

    revise(workload, failures, pull, number, &Tickets.revising/4)
  end

  defp act(workload, {:revise_review, findings, pull, number}, _notes) do
    failures =
      for %{lens: lens, finding: finding} <- findings do
        %{
          "name" => "#{lens} review: #{finding["title"]}",
          "result" => "needs_revision",
          "excerpt" => finding_text(finding)
        }
      end

    revise(workload, failures, pull, number, &Tickets.revising_for_review/4)
  end

  defp act(workload, {:revise_base, moved, pull, number}, _notes) do
    tell = fn workload, _names, attempt, max ->
      Tickets.revising_for_base(workload, length(moved["commits"]), attempt, max)
    end

    revise(workload, [], pull, number, tell, %{"baseMoved" => moved})
  end

  defp act(workload, verdict, notes), do: write(workload, verdict, notes)

  defp revise(workload, failures, pull, number, tell, extra \\ %{}) do
    attempt = attempt(workload) + 1

    revision =
      workload.workload
      |> Map.put("id", Ecto.UUID.generate())
      |> put_in(["repository", "baseCommit"], pull["head"]["sha"])
      |> Map.put(
        "revision",
        %{
          "pullRequestNumber" => to_string(number),
          "pullRequestUrl" => "https://github.com/#{workload.repository}/pull/#{number}",
          "branch" => pull["head"]["ref"],
          "attempt" => attempt,
          "failures" => failures
        }
        |> Map.merge(extra)
      )

    case Workloads.hand_on(workload, revision) do
      {:ok, _child} ->
        tell.(
          workload,
          Enum.map(failures, & &1["name"]),
          attempt - workload.budget_base,
          revisions()
        )

        :ok

      {:error, reason} ->
        {:error, "could not start a revision: #{inspect(reason)}"}
    end
  end

  defp finding_text(finding) do
    [
      "[#{finding["severity"]}] #{finding["title"]}",
      Finding.location_line(finding["location"]),
      "Evidence: #{finding["evidence"]}",
      if(finding["revision"], do: "What to revise: #{finding["revision"]}")
    ]
    |> Enum.reject(&is_nil/1)
    |> Enum.join("\n")
  end

  defp write(workload, {:failed, names, extra, :review}, _notes),
    do: write_to(Tickets.settled(workload, {:review_failed, names, extra}))

  defp write(workload, verdict, notes), do: write_to(Tickets.settled(workload, verdict, notes))

  defp write_to(result) do
    case result do
      :ok -> :ok
      {:error, message} -> {:error, "could not write to the ticket: #{message}"}
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

  # A check GitHub cancelled because its job reached its time limit is a failure of the job, and
  # not a cancellation: a re-run is cold again, finishes no more than the first did and times
  # out again, spending the repository's CI minutes to get what the rules forbid, a green by
  # trying again. GitHub says so only in the job's annotations ("The job has exceeded the
  # maximum execution time of 15m0s"), so they are read, and the check is treated as `timed_out`,
  # which is revised or failed like any other. Annotations that cannot be read leave it a
  # cancellation, as it was.
  defp timed_out(workload, %{"conclusion" => "cancelled", "id" => id} = run) do
    case GitHub.get("repos/#{workload.repository}/check-runs/#{id}/annotations") do
      {:ok, annotations} when is_list(annotations) ->
        if Enum.any?(annotations, &exceeded_time_limit?/1),
          do: Map.put(run, "conclusion", "timed_out"),
          else: run

      _ ->
        run
    end
  end

  defp timed_out(_workload, run), do: run

  defp exceeded_time_limit?(%{"message" => message}) when is_binary(message),
    do: message =~ ~r/exceeded the maximum execution time/i

  defp exceeded_time_limit?(_), do: false

  # The revisions used toward the budget: counted from the attempt the pull request had
  # reached when a maintainer last resumed it, so a resume grants a fresh budget.
  defp used(workload), do: attempt(workload) - workload.budget_base

  defp judge(workload, required, runs, pull_and_number) do
    # Every check on the commit counts, not only the ones the ruleset requires: a
    # failing check is investigated whether or not it blocks a merge (ADR 0024). The
    # required ones must also have appeared.
    deferred = deferred(workload)
    left_to = Enum.map(deferred, & &1.check)
    names = Enum.uniq(required ++ left_to ++ Map.keys(runs))
    looked_at = Enum.map(names, &{&1, runs[&1]})

    ended =
      for {name, run} <- looked_at,
          run != nil and run["status"] == "completed",
          do: {name, timed_out(workload, run)}

    bad = for {name, run} <- ended, run["conclusion"] not in @good, do: {name, run}
    cancelled = for {name, run} <- bad, run["conclusion"] == "cancelled", do: {name, run}
    failed = Enum.map(bad, &elem(&1, 0))

    unfinished =
      for {name, run} <- looked_at, is_nil(run) or run["status"] != "completed", do: name

    real = bad -- cancelled
    revisable = revisable(workload, pull_and_number)

    # The checks a gate was left to that no run of has appeared for.
    absent = for check <- Enum.uniq(left_to), runs[check] == nil, do: check

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

      # Only a check a gate was left to is missing, and it has had long enough to
      # appear: nothing has run that gate, so the ticket does not wait for ever.
      absent != [] and Enum.sort(unfinished) == Enum.sort(absent) and past_grace?(workload) ->
        {:ok, {:failed, Enum.sort(absent), never_appeared(deferred, absent)}}

      unfinished != [] ->
        {:waiting, "still waiting for: #{Enum.join(Enum.sort(unfinished), ", ")}"}

      true ->
        {:ok, :done}
    end
  end

  # How long a check a gate was left to has to appear after the workload ended.
  @deferred_grace_seconds 20 * 60

  defp past_grace?(%{finished_at: %DateTime{} = finished}),
    do: DateTime.diff(DateTime.utc_now(), finished) >= @deferred_grace_seconds

  defp past_grace?(_workload), do: false

  defp never_appeared(deferred, absent) do
    lines =
      for %{gate: gate, check: check} <- deferred, check in absent do
        "- the gate `#{gate}` was left to the check `#{check}`"
      end

    "The workspace could not run these gates, and the checks they were left to never appeared on the pull request within #{div(@deferred_grace_seconds, 60)} minutes of the workload ending, so nothing has run them:\n" <>
      Enum.join(lines, "\n")
  end

  # The gates the workspace left to a check on the pull request: each gate, the
  # check, and why it was not run in the workspace.
  defp deferred(workload) do
    for %{"name" => gate, "result" => %{"outcome" => "deferred", "ci_check" => check} = result} <-
          get_in(workload.record || %{}, ["gates"]) || [],
        is_binary(check) and check != "",
        do: %{gate: gate, check: check, reason: result["reason"]}
  end

  # What was left to CI is said on the ticket when it is done, because the check it
  # was left to has been seen to pass. A pull request that was merged before it was
  # looked at has not been checked, so nothing is claimed for it.
  defp with_left_to_ci(workload, %{"state" => "open", "merged" => merged}, verdict, notes)
       when merged != true do
    case {done?(verdict), deferred(workload)} do
      {true, [_ | _] = left} ->
        lines =
          for %{gate: gate, check: check, reason: reason} <- left do
            "- `#{gate}`, left to the check `#{check}`, which passed: #{reason}"
          end

        text = "Left to CI (not run in the workspace):\n" <> Enum.join(lines, "\n")
        if notes, do: notes <> "\n\n" <> text, else: text

      _ ->
        notes
    end
  end

  defp with_left_to_ci(_workload, _pull, _verdict, notes), do: notes

  defp done?(:done), do: true
  defp done?({:done, _}), do: true
  defp done?(_), do: false

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

      used(workload) >= revisions() ->
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
        "- revision #{index}: #{what}\n\n" <> Account.section(w.id)
      end

    {:failed, names,
     "Revisions made: #{used(workload)} of #{revisions()}.\n\n" <> Enum.join(lines, "\n\n")}
  end

  defp revisions,
    do: setting(:revisions)

  defp reruns, do: setting(:reruns)

  # A setting of settling, zero (none) unless set.
  defp setting(name),
    do: (Application.get_env(:control_plane, :settle) || []) |> Keyword.get(name, 0)

  defp automerge?(repository) do
    repository in ((Application.get_env(:control_plane, :settle) || [])[:automerge] || [])
  end
end
