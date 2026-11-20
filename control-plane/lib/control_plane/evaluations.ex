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
         {:ok, record} <- Workloads.create(with_evaluation(built, skill)),
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

  defp with_evaluation(workload, skill),
    do: Map.put(workload, "completeness", %{"skillDocument" => skill, "openItems" => []})

  defp settings, do: Application.get_env(:control_plane, :settle) || []
end
