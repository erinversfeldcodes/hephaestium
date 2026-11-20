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

  alias ControlPlane.{GitHub, Intake, Runner, Workload, Workloads}

  @athenaeum "erinversfeldcodes/athenaeum"
  @skill "ticket-completeness"
  @active ["queued", "preparing", "running", "gating"]

  @type result :: {:started, String.t()} | {:skipped, term()}

  @doc "Whether this repository's merged tickets are evaluated."
  @spec enabled?(String.t()) :: boolean()
  def enabled?(repository), do: repository in (settings()[:completeness] || [])

  @doc """
  Queue an evaluation of the ticket the merged workload was for, unless there is nothing
  to judge: the repository has not opted in, the ticket is already closed, or an
  evaluation of it is already under way.
  """
  @spec after_merge(%Workload{}) :: result()
  def after_merge(%Workload{} = merged) do
    repository = merged.repository
    number = merged.ticket_number

    with :ok <- enabled(repository),
         :ok <- open(repository, number),
         :ok <- not_evaluating(repository, number),
         {:ok, skill} <- skill(),
         {:ok, built} <- Intake.build(repository, number, approval(merged)),
         {:ok, record} <-
           Workloads.create(with_evaluation(built, skill, open_items(repository, number))),
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

      if owed?(latest) and length(evaluations) < 1 + rechecks() and waited?(latest, now),
        do: [after_merge(latest)],
        else: []
    end)
  end

  defp owed?(%Workload{state: "completed", record: %{"completeness" => %{"verdict" => "owed"}}}),
    do: true

  defp owed?(_), do: false

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
    case GitHub.get("repos/#{repository}/issues/#{number}") do
      {:ok, %{"state" => "open"}} -> :ok
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

  defp evaluation?(%Workload{workload: %{"completeness" => _}}), do: true
  defp evaluation?(_), do: false

  # The skill as the pinned release has it.
  defp skill do
    with {:ok, ref} <- athenaeum_ref(),
         {:ok, text} <-
           GitHub.raw("repos/#{@athenaeum}/contents/skills/#{@skill}/SKILL.md?ref=#{ref}") do
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

  defp with_evaluation(workload, skill, open_items),
    do: Map.put(workload, "completeness", %{"skillDocument" => skill, "openItems" => open_items})

  defp settings, do: Application.get_env(:control_plane, :settle) || []
end
