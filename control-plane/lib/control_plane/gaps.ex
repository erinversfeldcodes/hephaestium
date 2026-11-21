defmodule ControlPlane.Gaps do
  @moduledoc """
  Working out what is missing from a ticket that is not complete, by questioning the person.

  When a verdict finds a ticket's goal not met, or could not be judged, Hephaestium takes one
  step: a read-only workload that runs the `close-the-gap` skill from the pinned Athenaeum
  release, reads the ticket's thread, and ends by asking the next question, proposing the
  tickets that close the gap, or saying there is nothing more to ask. What it wrote is a
  comment on the ticket. A person's reply starts the next step, so a question that waits
  costs nothing. A proposal is made into tickets under the ticket, each waiting for what it
  depends on; none is approved, because a person's approval label on each is what starts the
  work.
  """

  alias ControlPlane.{Evaluations, GitHub, Intake, Runner, Workload, Workloads}

  @skill "close-the-gap"
  @active ["queued", "preparing", "running", "gating"]
  @own "**Hephaestium:**"

  # However a step was asked for, a ticket takes at most this many in a day: each costs an
  # agent run and a comment.
  @daily_limit 6

  @type result :: {:started, String.t()} | {:skipped, term()}

  @doc "Take a step for this ticket, saying why."
  @spec start(String.t(), integer(), String.t()) :: result()
  def start(repository, number, because) do
    with :ok <- enabled(repository),
         :ok <- open(repository, number),
         :ok <- not_stepping(repository, number),
         :ok <- not_asked_too_often(repository, number),
         {:ok, approval} <- approval(repository, number),
         {:ok, skill} <- Evaluations.skill(@skill),
         {:ok, built} <- Intake.build(repository, number, approval),
         {:ok, record} <-
           Workloads.create(
             Map.put(built, "gap", %{"skillDocument" => skill, "because" => because})
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
  A verdict has ended. If it found the goal not met, or could not judge it, start the
  questioning; any other verdict leaves nothing to ask.
  """
  @spec after_verdict(%Workload{}, map()) :: result() | :none
  def after_verdict(%Workload{} = evaluation, %{"verdict" => "not_met"}),
    do:
      start(evaluation.repository, evaluation.ticket_number, "the verdict found the goal not met")

  def after_verdict(%Workload{} = evaluation, %{"verdict" => "cannot_judge"}),
    do:
      start(
        evaluation.repository,
        evaluation.ticket_number,
        "the goal could not be judged, so what was out of reach has to be found out"
      )

  def after_verdict(_evaluation, _verdict), do: :none

  @doc """
  Question each ticket whose latest verdict found the goal not met, or could not be judged,
  and which has had no step since. A verdict starts the questioning as it ends, so this
  finds the ones that did not: those judged before the questioning existed, or when it could
  not be started.
  """
  @spec unquestioned() :: [result()]
  def unquestioned do
    (Application.get_env(:control_plane, :settle)[:completeness] || [])
    |> Workloads.evaluations()
    |> Enum.group_by(&{&1.repository, &1.ticket_number})
    |> Enum.flat_map(fn {{repository, number}, evaluations} ->
      latest = List.last(evaluations)

      with %Workload{
             state: "completed",
             record: %{"completeness" => verdict},
             finished_at: %DateTime{}
           } <-
             latest,
           false <- stepped_since?(repository, number, latest),
           result when result != :none <- after_verdict(latest, verdict) do
        [result]
      else
        _ -> []
      end
    end)
  end

  @doc """
  Take a step for each ticket Hephaestium closed on its evidence that a person has since
  reopened: a person who reopens it is saying the verdict was wrong. It is looked for among
  the tickets of repositories that let Hephaestium close, whose latest verdict was one that
  closes, and which have had no step since that verdict.
  """
  @spec reopened() :: [result()]
  def reopened do
    (Application.get_env(:control_plane, :settle)[:closures] || [])
    |> Workloads.evaluations()
    |> Enum.group_by(&{&1.repository, &1.ticket_number})
    |> Enum.flat_map(fn {{repository, number}, evaluations} ->
      latest = List.last(evaluations)

      if closed_on_evidence?(latest) and not stepped_since?(repository, number, latest),
        do: reopened_since(repository, number, latest),
        else: []
    end)
  end

  defp closed_on_evidence?(
         %Workload{
           state: "completed",
           record: %{"completeness" => verdict},
           finished_at: %DateTime{}
         } =
           evaluation
       )
       when is_map(verdict),
       do: Evaluations.closes?(verdict) and not Evaluations.epic?(evaluation)

  defp closed_on_evidence?(_), do: false

  defp stepped_since?(repository, number, %Workload{finished_at: ended}) do
    repository
    |> Workloads.for_ticket(number)
    |> Enum.any?(
      &(step?(&1) and
          DateTime.compare(DateTime.from_naive!(&1.inserted_at, "Etc/UTC"), ended) != :lt)
    )
  end

  defp reopened_since(repository, number, %Workload{finished_at: ended}) do
    case GitHub.get("repos/#{repository}/issues/#{number}/events?per_page=100") do
      {:ok, events} when is_list(events) ->
        if Enum.any?(events, &reopened_after?(&1, ended)),
          do: [
            start(
              repository,
              number,
              "the ticket was reopened after Hephaestium closed it on its evidence, so the verdict was wrong"
            )
          ],
          else: []

      {:ok, _} ->
        []

      {:error, message} ->
        [{:skipped, {:github_error, message}}]
    end
  end

  defp reopened_after?(%{"event" => "reopened", "created_at" => at}, ended) when is_binary(at) do
    case DateTime.from_iso8601(at) do
      {:ok, at, _} -> DateTime.compare(at, ended) == :gt
      _ -> false
    end
  end

  defp reopened_after?(_event, _ended), do: false

  @doc """
  Take the next step for each ticket whose last step is waiting on a person who has since
  said something: its last step asked a question, or failed, and a comment that is not
  Hephaestium's own was made after it ended.
  """
  @spec replies() :: [result()]
  def replies do
    (Application.get_env(:control_plane, :settle)[:completeness] || [])
    |> Workloads.steps()
    |> Enum.group_by(&{&1.repository, &1.ticket_number})
    |> Enum.flat_map(fn {{repository, number}, steps} ->
      latest = List.last(steps)

      if waiting?(latest), do: answered(repository, number, latest), else: []
    end)
  end

  defp waiting?(%Workload{state: state}) when state in ["failed", "timed_out"], do: true

  defp waiting?(%Workload{state: "completed", record: %{"gap" => %{"step" => "question"}}}),
    do: true

  defp waiting?(_), do: false

  defp answered(repository, number, %Workload{finished_at: %DateTime{} = ended}) do
    case GitHub.get("repos/#{repository}/issues/#{number}/comments?per_page=100") do
      {:ok, comments} when is_list(comments) ->
        if Enum.any?(comments, &replied_after?(&1, ended)),
          do: [start(repository, number, "a person replied")],
          else: []

      {:ok, _} ->
        []

      {:error, message} ->
        [{:skipped, {:github_error, message}}]
    end
  end

  defp answered(_repository, _number, _step), do: []

  # A person's comment is any that Hephaestium did not write: it comments under the
  # person's own login, so its comments are told apart by how they begin.
  defp replied_after?(%{"created_at" => at, "body" => body}, ended) when is_binary(body) do
    with {:ok, at, _} <- DateTime.from_iso8601(at) do
      DateTime.compare(at, ended) == :gt and not String.starts_with?(body, @own)
    else
      _ -> false
    end
  end

  defp replied_after?(_comment, _ended), do: false

  @doc """
  Make a proposal into tickets: each is created, then made a sub-issue of the ticket it is
  under, and made to wait for the tickets it depends on. Returns the tickets made, as
  `{title, number}`, and what could not be done.
  """
  @spec create([map()], String.t()) :: {[{String.t(), integer()}], [String.t()]}
  def create(tickets, repository) do
    made =
      Enum.map(tickets, fn ticket ->
        {ticket,
         GitHub.write("POST", "repos/#{repository}/issues", [
           "title=" <> ticket["title"],
           "body=" <> ticket["body"]
         ])}
      end)

    by_title =
      for {ticket, {:ok, %{"number" => number, "id" => id}}} <- made,
          into: %{},
          do: {ticket["title"], {number, id}}

    problems =
      for {ticket, result} <- made, not match?({:ok, %{"number" => _, "id" => _}}, result) do
        "#{ticket["title"]} could not be created"
      end

    relations =
      for {ticket, _} <- made, {number, id} <- [by_title[ticket["title"]]], number != nil do
        link(repository, ticket, number, id, by_title)
      end

    {Enum.map(by_title, fn {title, {number, _}} -> {title, number} end)
     |> Enum.sort_by(fn {_, number} -> number end), problems ++ List.flatten(relations)}
  end

  defp link(repository, ticket, number, id, by_title) do
    parent = ticket["parent"] |> to_string() |> String.to_integer()

    under =
      case GitHub.write("POST", "repos/#{repository}/issues/#{parent}/sub_issues", [
             {:typed, "sub_issue_id=#{id}"}
           ]) do
        {:ok, _} -> []
        {:error, message} -> ["#{ticket["title"]} could not be made a sub-issue: #{message}"]
      end

    waits =
      for title <- ticket["dependsOn"] || [], {_, dependency_id} <- [by_title[title]] do
        case GitHub.write(
               "POST",
               "repos/#{repository}/issues/#{number}/dependencies/blocked_by",
               [
                 {:typed, "issue_id=#{dependency_id}"}
               ]
             ) do
          {:ok, _} -> []
          {:error, message} -> ["#{ticket["title"]} could not be made to wait: #{message}"]
        end
      end

    under ++ List.flatten(waits)
  end

  defp enabled(repository),
    do: if(Evaluations.enabled?(repository), do: :ok, else: {:skip, :not_enabled})

  defp open(repository, number) do
    case GitHub.get("repos/#{repository}/issues/#{number}") do
      {:ok, %{"state" => "open"}} -> :ok
      {:ok, _} -> {:skip, :ticket_closed}
      {:error, message} -> {:github_error, message}
    end
  end

  defp not_stepping(repository, number) do
    stepping? =
      repository
      |> Workloads.for_ticket(number)
      |> Enum.any?(&(step?(&1) and &1.state in @active))

    if stepping?, do: {:skip, :already_stepping}, else: :ok
  end

  defp not_asked_too_often(repository, number) do
    since = NaiveDateTime.add(NaiveDateTime.utc_now(), -86_400, :second)

    count =
      repository
      |> Workloads.for_ticket(number)
      |> Enum.count(&(step?(&1) and NaiveDateTime.compare(&1.inserted_at, since) != :lt))

    if count >= @daily_limit, do: {:skip, :asked_too_often}, else: :ok
  end

  defp step?(%Workload{workload: %{"gap" => _}}), do: true
  defp step?(_), do: false

  # Who approved the ticket, from the latest workload that carries it.
  defp approval(repository, number) do
    repository
    |> Workloads.for_ticket(number)
    |> Enum.filter(&match?(%{"approval" => %{"approvedBy" => _}}, &1.workload))
    |> List.last()
    |> case do
      %Workload{workload: %{"approval" => approval}} ->
        {:ok, %{login: approval["approvedBy"], text: approval["approvedAt"]}}

      nil ->
        {:skip, :no_approval}
    end
  end
end
