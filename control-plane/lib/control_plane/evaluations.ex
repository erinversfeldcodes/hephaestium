defmodule ControlPlane.Evaluations do
  @moduledoc """
  Judging whether a ticket's goal is met, once the change that was made for it has merged.

  A merged and green pull request says a change is ready, not that the ticket is
  finished. When a ticket's pull request has merged and the ticket is still open, for a
  repository that opted in (`HEPH_COMPLETENESS`), an evaluation is queued: a workload that
  runs the `ticket-completeness` skill, from the pinned Athenaeum release and never from
  anywhere a pull request can change, in a read-only workspace at the default branch's
  current tip, and ends with a verdict.

  The workload is built the way a ticket's first one is, by `heph`, so it carries the
  ticket as it reads now and the commit the default branch is at now, not what they were
  when the ticket was approved.
  """

  alias ControlPlane.{CodeHost, Intake, Runner, Tracker, Workload, Workloads}

  @athenaeum "erinversfeldcodes/athenaeum"
  @skill "ticket-completeness"
  @active ["queued", "preparing", "running", "gating"]

  @type result :: {:started, String.t()} | {:skipped, term()}

  @doc "Whether this repository's merged tickets are evaluated."
  @spec enabled?(String.t()) :: boolean()
  def enabled?(repository), do: repository in (settings()[:completeness] || [])

  @doc "Whether this repository lets Hephaestium close a ticket whose verdict the evidence decides."
  @spec closing?(String.t()) :: boolean()
  def closing?(repository), do: repository in (settings()[:closures] || [])

  @doc """
  Whether a verdict closes its ticket. The contract's `decides` already says so; it is the
  agent's word, so it is checked here against what the verdict itself holds: the verdict is
  met, the ticket is not an epic, nothing is owed or left unchecked, and every requirement is
  met, can be settled by evidence, and rests on evidence that was observed or tested, not on
  the code existing or a claim. Anything short of that is for a person.
  """
  @spec closes?(map()) :: boolean()
  def closes?(%{"verdict" => "met", "decides" => "evidence", "epic" => false} = verdict) do
    requirements = verdict["requirements"]

    verdict["owed"] in [nil, []] and verdict["could_not_check"] in [nil, []] and
      is_list(requirements) and requirements != [] and Enum.all?(requirements, &settled?/1)
  end

  def closes?(_), do: false

  defp settled?(%{
         "status" => "met",
         "settled_by" => "evidence",
         "evidence" => %{"kind" => kind}
       }),
       do: kind in ["observed", "test"]

  defp settled?(_), do: false

  @doc """
  Whether the tracker records sub-issues under the ticket a workload is for. That makes it an
  epic whatever an agent's verdict says of it: an epic is judged complete by a person.
  """
  @spec epic?(%Workload{}) :: boolean()
  def epic?(%Workload{workload: %{"ticket" => %{"subIssues" => count}}}) when is_number(count),
    do: count > 0

  def epic?(_), do: false

  @doc """
  Queue an evaluation of the ticket the merged workload was for, unless there is nothing
  to judge: the repository has not opted in, the ticket is already closed, or an
  evaluation of it is already under way.
  """
  @spec after_merge(%Workload{}, %{login: String.t(), text: String.t()} | nil) :: result()
  def after_merge(%Workload{} = merged, approval \\ nil), do: start(merged, approval, "")

  # A verdict the contract refused, or left in no usable form, is an evaluation that failed
  # for a fault the agent can correct. These are what the executor says then.
  @refusals [
    "the verdict does not pass the completeness contract",
    "the verdict is not JSON",
    "the evaluation left no verdict",
    "the verdict is for"
  ]

  @doc """
  Whether an evaluation failed because its verdict was refused, and was a first attempt.
  Only that is tried again, and only once: a failure of the agent or the runtime is not
  something a second try is shown to help, and a second refusal is for a person to see.
  """
  @spec retryable?(%Workload{}) :: boolean()
  def retryable?(%Workload{state: "failed", reason: reason, workload: %{"completeness" => c}})
      when is_binary(reason) do
    c["refused"] in [nil, ""] and Enum.any?(@refusals, &String.starts_with?(reason, &1))
  end

  def retryable?(_), do: false

  @doc """
  Try an evaluation again, told why its verdict was refused. It is built for the same
  approval, and is held to the same limits as any evaluation.
  """
  @spec retry(%Workload{}) :: result()
  def retry(%Workload{workload: %{"approval" => _}} = failed),
    do: start(failed, approval(failed), failed.reason || "")

  defp start(%Workload{} = merged, approval, refused) do
    repository = merged.repository
    number = merged.ticket_number

    with :ok <- enabled(repository),
         :ok <- open(repository, number),
         :ok <- not_evaluating(repository, number),
         approval = approval || approval(merged),
         :ok <- not_judged_too_often(repository, number, approval.text),
         {:ok, skill} <- skill(),
         {:ok, built} <- Intake.build(repository, number, approval),
         {:ok, record} <-
           Workloads.create(
             with_evaluation(built, skill, open_items(repository, number), refused)
           ),
         {:ok, _job} <- Runner.enqueue(record.id) do
      {:started, record.id}
    else
      {:skip, reason} -> {:skipped, reason}
      {:github_error, message} -> {:skipped, {:github_error, message}}
      {:could_not_build, message} -> {:skipped, {:could_not_build, message}}
      {:no_skill, message} -> {:skipped, {:no_skill, message}}
      other -> {:skipped, {:unexpected, inspect(other)}}
    end
  end

  @doc """
  Judge the tickets whose pull request merged after the ticket was settled. A pull request
  is settled when it is green, and a person merges it later, so settling alone never sees
  the merge. This looks again, for the tickets settled in the last fortnight that have not
  been judged, and asks for a judgement of each whose pull request has merged.
  """
  @spec merged(DateTime.t()) :: [result()]
  def merged(now \\ DateTime.utc_now()) do
    repositories = settings()[:completeness] || []

    judged =
      repositories
      |> Workloads.evaluations()
      |> MapSet.new(&{&1.repository, &1.ticket_number})

    repositories
    |> Workloads.settled_with_pull_request(DateTime.add(now, -14 * 86_400, :second))
    |> Enum.reject(&MapSet.member?(judged, {&1.repository, &1.ticket_number}))
    |> Enum.flat_map(&look_for_merge/1)
  end

  defp look_for_merge(workload) do
    with {:ok, number} <- Workloads.pull_request_number(workload) do
      case CodeHost.change(workload.repository, number) do
        {:ok, %{merged: true}} -> [after_merge(workload)]
        {:ok, _} -> []
        {:error, message} -> [{:skipped, {:github_error, message}}]
      end
    else
      _ -> []
    end
  end

  @doc """
  Judge the epics whose sub-issues are all closed. An epic is complete across its parts, which
  no single pull request shows, so it is judged when its last sub-issue closes, however that
  happened, and again if one closes after the last judgement. The judgement is of a ticket a
  maintainer approved only through the work beneath it, so it is built for the approval of the
  latest sub-issue Hephaestium worked. A person is the one to judge it complete.
  """
  @spec epics() :: [result()]
  def epics, do: Enum.flat_map(settings()[:completeness] || [], &epics_of/1)

  defp epics_of(repository) do
    case Tracker.open_epics(repository) do
      {:ok, tickets} ->
        tickets
        |> Enum.filter(&finished_epic?/1)
        |> Enum.flat_map(&roll_up(repository, &1.key))

      {:error, message} ->
        [{:skipped, {:github_error, message}}]
    end
  end

  defp finished_epic?(%{progress: %{total: total, completed: done}})
       when is_integer(total) and total > 0,
       do: done == total

  defp finished_epic?(_), do: false

  defp roll_up(repository, number) do
    with {:ok, children} <-
           Tracker.sub_tickets(repository, number),
         {:ok, approval} <- child_approval(repository, children),
         :ok <- not_judged_since_last_closed(repository, number, children) do
      [start(%Workload{repository: repository, ticket_number: number}, approval, "")]
    else
      {:error, message} -> [{:skipped, {:github_error, message}}]
      {:skip, :judged} -> []
      {:skip, reason} -> [{:skipped, reason}]
    end
  end

  defp child_approval(repository, children) do
    children
    |> Enum.flat_map(&Workloads.for_ticket(repository, &1.key))
    |> Enum.filter(&match?(%{"approval" => %{"approvedBy" => _}}, &1.workload))
    |> List.last()
    |> case do
      nil ->
        {:skip, :no_approval}

      %Workload{workload: %{"approval" => approval}} ->
        {:ok, %{login: approval["approvedBy"], text: approval["approvedAt"]}}
    end
  end

  # What closed last decides whether an earlier judgement still stands.
  defp not_judged_since_last_closed(repository, number, children) do
    closed =
      children
      |> Enum.map(& &1.closed_at)
      |> Enum.filter(&match?(%DateTime{}, &1))
      |> Enum.max(DateTime, fn -> nil end)

    judged =
      repository
      |> Workloads.for_ticket(number)
      |> Enum.filter(&evaluation?/1)
      |> Enum.map(&DateTime.from_naive!(&1.inserted_at, "Etc/UTC"))

    cond do
      closed == nil -> {:skip, :no_closure_seen}
      Enum.any?(judged, &(DateTime.compare(&1, closed) != :lt)) -> {:skip, :judged}
      true -> :ok
    end
  end

  @doc """
  Judge again the tickets whose last verdict was owed. A ticket is judged again when its
  latest evaluation ended with a verdict that something is owed, a day (configurable) has
  passed since it ended, and it has not been judged more often than the limit allows. What
  was owed goes into the next evaluation, so it is told which items are open. A ticket that
  has since closed, or whose latest evaluation is still under way, starts nothing.
  """
  @spec recheck(DateTime.t()) :: [result()]
  def recheck(now \\ DateTime.utc_now()) do
    (settings()[:completeness] || [])
    |> Workloads.evaluations()
    |> Enum.group_by(&{&1.repository, &1.ticket_number})
    |> Enum.flat_map(fn {_ticket, evaluations} ->
      latest = List.last(evaluations)

      if (owed?(latest) or failed?(latest)) and length(evaluations) < 1 + rechecks() and
           waited?(latest, now),
         do: [after_merge(latest)],
         else: []
    end)
  end

  defp owed?(%Workload{state: "completed", record: %{"completeness" => %{"verdict" => "owed"}}}),
    do: true

  defp owed?(_), do: false

  # An evaluation that ended without a verdict judged nothing, so the ticket is still
  # unjudged; it is tried again on the same schedule and under the same limits.
  defp failed?(%Workload{state: state}), do: state in ["failed", "timed_out"]

  # An owed verdict waits for the day its measurement needs. A failed evaluation waits for
  # nothing, so it is tried again soon, under the same limits.
  @failure_wait_seconds 3600

  defp waited?(%Workload{state: state, finished_at: %DateTime{} = at}, now)
       when state in ["failed", "timed_out"],
       do: DateTime.diff(now, at, :second) >= @failure_wait_seconds

  defp waited?(%Workload{finished_at: %DateTime{} = at}, now),
    do: DateTime.diff(now, at, :second) >= interval_hours() * 3600

  defp waited?(_, _), do: false

  # What the latest evaluation of the ticket said is owed, as the next one is told it.
  defp open_items(repository, number) do
    repository
    |> Workloads.for_ticket(number)
    |> Enum.filter(&evaluation?/1)
    |> List.last()
    |> items()
  end

  defp items(%Workload{
         state: "completed",
         record: %{"completeness" => %{"verdict" => "owed", "owed" => owed}}
       })
       when is_list(owed),
       do: Enum.map(owed, &item/1)

  defp items(_), do: []

  defp item(owed) do
    %{
      "what" => owed["what"],
      "run" => owed["run"],
      "when" => owed["when"],
      "settledBy" => owed["settled_by"]
    }
  end

  defp interval_hours, do: settings()[:completeness_interval_hours] || 24
  defp rechecks, do: settings()[:completeness_rechecks] || 7

  defp enabled(repository),
    do: if(enabled?(repository), do: :ok, else: {:skip, :not_enabled})

  defp open(repository, number) do
    case Tracker.ticket(repository, number) do
      {:ok, %{state: :open}} -> :ok
      {:ok, _} -> {:skip, :ticket_closed}
      {:error, message} -> {:github_error, message}
    end
  end

  defp not_evaluating(repository, number) do
    evaluating? =
      repository
      |> Workloads.for_ticket(number)
      |> Enum.any?(&(evaluation?(&1) and &1.state in @active))

    if evaluating?, do: {:skip, :already_evaluating}, else: :ok
  end

  # Under one approval, a ticket is judged at most this many times in a day. A judgement costs
  # an agent run and writes a comment, so a path that asks again and again, as one did for as
  # long as an approval label stood, must stop by itself. The count is for the approval and not
  # for the ticket: a person approving a ticket again is a deliberate act that starts a count of
  # its own, and the evaluations an earlier approval ran must not hold it.
  @daily_limit 6

  defp not_judged_too_often(repository, number, approved_at) do
    since = NaiveDateTime.add(NaiveDateTime.utc_now(), -86_400, :second)

    count =
      repository
      |> Workloads.for_ticket(number)
      |> Enum.count(
        &(evaluation?(&1) and approved?(&1, approved_at) and
            NaiveDateTime.compare(&1.inserted_at, since) != :lt)
      )

    if count >= @daily_limit, do: {:skip, :judged_too_often}, else: :ok
  end

  defp approved?(%Workload{workload: %{"approval" => %{"approvedAt" => at}}}, approved_at),
    do: at == approved_at

  defp approved?(_, _), do: false

  defp evaluation?(%Workload{workload: %{"completeness" => _}}), do: true
  defp evaluation?(_), do: false

  # The skill as the pinned release has it.
  defp skill, do: skill(@skill)

  @doc "The text of a skill as the pinned Athenaeum release has it."
  @spec skill(String.t()) :: {:ok, String.t()} | {:no_skill, String.t()}
  def skill(name) do
    with {:ok, ref} <- athenaeum_ref(),
         {:ok, text} <-
           CodeHost.file(@athenaeum, "skills/#{name}/SKILL.md", ref) do
      {:ok, text}
    else
      {:error, message} -> {:no_skill, message}
    end
  end

  defp athenaeum_ref do
    case settings()[:athenaeum_ref] do
      ref when is_binary(ref) and ref != "" ->
        {:ok, ref}

      _ ->
        {:error, "HEPH_ATHENAEUM_REF is not set, so there is no release to read the skill from"}
    end
  end

  # Who approved the ticket and when, as the workload that was made for it holds it: the
  # evaluation is of a ticket a maintainer approved.
  defp approval(%Workload{workload: %{"approval" => approval}}),
    do: %{login: approval["approvedBy"], text: approval["approvedAt"]}

  # `refused` is there only on a second attempt.
  defp with_evaluation(workload, skill, open_items, refused) do
    evaluation = %{"skillDocument" => skill, "openItems" => open_items}
    evaluation = if refused == "", do: evaluation, else: Map.put(evaluation, "refused", refused)
    Map.put(workload, "completeness", evaluation)
  end

  defp settings, do: Application.get_env(:control_plane, :settle) || []
end
