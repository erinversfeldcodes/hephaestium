defmodule ControlPlane.Intake do
  @moduledoc """
  Finding the tickets a maintainer has approved, and starting a workload for each.

  A ticket is approved by a label, `heph:ready`, applied by someone with write
  access to the repository (ADR 0020). The approval stands only while no one
  without write access has edited the ticket since it was given, because what the
  maintainer approved is the ticket as they read it. The ticket's workload is
  built by `heph workload`, with the maintainer who labelled it as the approver
  and the time they did it, so that the logic that builds a workload is the one
  `heph run` uses.

  Intake fails closed: anything GitHub or `heph` cannot say, or says an error
  about, is not an approval.
  """

  alias ControlPlane.{
    CodeHost,
    Evaluations,
    Prepush,
    Programs,
    Runner,
    Tickets,
    Tracker,
    Workloads
  }

  @label "heph:ready"
  @writers [:admin, :maintain, :write]

  @type result :: :started | :resumed | {:evaluating, String.t()} | {:skipped, term()}

  @doc "Check every approved ticket of a repository, and start a workload for each that stands."
  @spec run(String.t()) :: [%{ticket: integer(), result: result()}] | {:error, String.t()}
  def run(repo) do
    case Tracker.approved_tickets(repo, @label) do
      {:ok, tickets} ->
        for ticket <- tickets do
          %{ticket: ticket.key, result: consider(repo, ticket)}
        end

      {:error, message} ->
        {:error, message}
    end
  end

  defp consider(repo, ticket) do
    number = ticket.key

    with {:ok, events} <- fetch(Tracker.ticket_history(repo, number, pages: :all)),
         {:ok, approval} <- approval(events),
         {:ok, true} <- writer(repo, approval.login, :approver_cannot_write),
         :ok <- not_started(repo, number, approval.at),
         :ok <- not_edited_since(repo, number, events, approval),
         {:ok, plan} <- plan(repo, number) do
      act(plan, repo, ticket, approval)
    else
      {:ok, false, reason} -> {:skipped, reason}
      {:skip, reason} -> {:skipped, reason}
      {:github_error, message} -> {:skipped, {:github_error, message}}
      {:could_not_build, message} -> {:skipped, {:could_not_build, message}}
      other -> {:skipped, {:unexpected, inspect(other)}}
    end
  end

  # A new workload, unless the ticket's pull request is open: then that pull request is
  # resumed (#131). A ticket whose pull request is merged, closed, or not one the runtime
  # made is left to a person: a new run would collide with its branch.
  defp plan(repo, number) do
    case Workloads.pull_request_workload(repo, number) do
      nil ->
        {:ok, :build}

      workload ->
        with {:ok, pull_number} <- pull_request_number(workload),
             {:ok, pull} <- fetch(CodeHost.change(repo, pull_number)) do
          case resumable(repo, workload, pull) do
            :ok -> {:ok, {:resume, workload, pull}}
            :evaluate -> {:ok, {:evaluate, workload}}
            {:skip, _} = skip -> skip
          end
        end
    end
  end

  defp pull_request_number(workload) do
    case Workloads.pull_request_number(workload) do
      {:ok, number} -> {:ok, number}
      {:error, _} -> {:skip, :not_a_runtime_pull_request}
    end
  end

  # A pull request that merged leaves nothing to resume. For a repository that judges its
  # merged tickets, a re-approval asks for that judgement instead: whether the ticket the
  # pull request was for is finished, or what is still owed.
  defp resumable(repo, _workload, %{merged: true}),
    do: if(Evaluations.enabled?(repo), do: :evaluate, else: {:skip, :pull_request_merged})

  defp resumable(_repo, _workload, %{state: :closed}), do: {:skip, :pull_request_closed}

  defp resumable(repo, workload, %{state: :open} = change) do
    if Workloads.runtime_branch?(workload, change.head_ref) and change.head_repository == repo,
      do: :ok,
      else: {:skip, :not_a_runtime_pull_request}
  end

  defp resumable(_repo, _workload, _pull), do: {:skip, :not_a_runtime_pull_request}

  defp act(:build, repo, ticket, approval) do
    number = ticket.key

    with {:ok, workload} <- build(repo, number, approval),
         :ok <- unblocked(workload),
         {:ok, workload} <- Prepush.add(judged_afterwards(workload, repo), repo),
         {:ok, record} <- create(workload),
         {:ok, _job} <- Runner.enqueue(record.id) do
      :started
    else
      {:error, message} when is_binary(message) -> {:skipped, {:could_not_build, message}}
      {:blocked, numbers} -> {:skipped, {:blocked_by, numbers}}
      {:github_error, message} -> {:skipped, {:github_error, message}}
      {:could_not_build, message} -> {:skipped, {:could_not_build, message}}
      other -> {:skipped, {:unexpected, inspect(other)}}
    end
  end

  defp act({:resume, workload, change}, _repo, ticket, approval) do
    :ok = Workloads.resume(workload, approval.at)
    _ = Tickets.resumed(workload, ticket.labels, approval.login, change.url)
    :resumed
  end

  # The judgement is built for this approval, not the ticket's first one: a pass that sees an
  # approval looks for a workload that carries it, and without one asks again.
  defp act({:evaluate, workload}, _repo, _issue, approval) do
    case Evaluations.after_merge(workload, %{login: approval.login, text: approval.text}) do
      {:started, id} -> {:evaluating, id}
      {:skipped, reason} -> {:skipped, reason}
    end
  end

  # A ticket that an open ticket blocks is not started: the work it waits for is not
  # there yet, and a run now would build on ground that is about to change. The ticket
  # stays approved, so it starts on a later pass once nothing open blocks it. The workload
  # carries the numbers as protobuf's JSON gives them, which is strings.
  defp unblocked(workload) do
    case get_in(workload, ["ticket", "blockedBy"]) do
      [_ | _] = blockers -> {:blocked, Enum.map(blockers, &blocker_number/1)}
      _ -> :ok
    end
  end

  defp blocker_number(number) when is_integer(number), do: number
  defp blocker_number(number) when is_binary(number), do: String.to_integer(number)

  defp fetch({:ok, value}), do: {:ok, value}
  defp fetch({:error, message}), do: {:github_error, message}

  # Who labelled the ticket and when: the most recent labelling counts.
  defp approval(events) do
    events
    |> Enum.filter(&(&1.kind == :labeled and &1.label == @label))
    |> Enum.flat_map(fn event ->
      with login when is_binary(login) <- event.actor,
           %DateTime{} = at <- event.at do
        [%{login: login, at: at, text: event.at_text}]
      else
        _ -> []
      end
    end)
    |> case do
      [] -> {:skip, :no_approval_event}
      approvals -> {:ok, Enum.max_by(approvals, & &1.at, DateTime)}
    end
  end

  # Whether a login may write to the repository.
  defp writer(repo, login, reason) do
    case CodeHost.permission(repo, login) do
      {:ok, permission} when permission in @writers -> {:ok, true}
      {:ok, _} -> {:ok, false, reason}
      {:error, message} -> {:github_error, message}
    end
  end

  defp resumed_for?(%{resumed_for: %DateTime{} = given}, at),
    do: DateTime.compare(given, at) == :eq

  defp resumed_for?(_workload, _at), do: false

  defp not_started(repo, number, at) do
    started? =
      repo
      |> Workloads.for_ticket(number)
      |> Enum.any?(fn workload ->
        with text when is_binary(text) <- get_in(workload.workload, ["approval", "approvedAt"]),
             {:ok, given, _} <- DateTime.from_iso8601(text) do
          DateTime.compare(given, at) == :eq
        else
          _ -> false
        end or resumed_for?(workload, at)
      end)

    if started?, do: {:skip, :already_started}, else: :ok
  end

  # An edit after the approval by someone who cannot write voids it: the maintainer
  # approved the ticket as they read it. An editor GitHub does not name cannot be
  # shown to be a maintainer.
  defp not_edited_since(repo, number, events, approval) do
    renamers =
      for event <- events,
          event.kind == :renamed,
          after?(event.at, approval.at),
          do: event.actor

    with {:ok, edits} <- fetch(Tracker.ticket_edits(repo, number)) do
      editors = for edit <- edits, after?(edit.at, approval.at), do: edit.editor

      editors_who_cannot_write(repo, Enum.uniq(renamers ++ editors))
    end
  end

  # :ok when every editor can write; the skip when one cannot, or is not named;
  # and an error when GitHub could not say whether one can.
  defp editors_who_cannot_write(repo, editors) do
    Enum.reduce_while(editors, :ok, fn
      nil, _ ->
        {:halt, {:skip, :edited_after_approval}}

      login, :ok ->
        case writer(repo, login, :edited_by_non_writer) do
          {:ok, true} -> {:cont, :ok}
          {:ok, false, _} -> {:halt, {:skip, :edited_after_approval}}
          {:github_error, message} -> {:halt, {:github_error, message}}
        end
    end)
  end

  # A time that was not given, or cannot be read, counts as after: it cannot be shown to be before.
  defp after?(%DateTime{} = time, at), do: DateTime.compare(time, at) == :gt
  defp after?(_, _), do: true

  @doc """
  Build the workload `heph` makes for a ticket and an approval, from the ticket as it reads
  now and the default branch's tip now. Another caller than intake needs one: judging a
  merged ticket starts from the ticket as it is, not as it was approved.
  """
  @spec build(String.t(), integer(), %{login: String.t(), text: String.t()}) ::
          {:ok, map()} | {:could_not_build, String.t()}
  def build(repo, number, approval) do
    arguments = [
      "workload",
      Integer.to_string(number),
      "--repo",
      repo,
      "--approver",
      approval.login,
      "--approved-at",
      approval.text
    ]

    if Programs.allowed?(heph()) do
      run_heph(arguments, number, approval)
    else
      {:could_not_build, Programs.refusal(heph())}
    end
  rescue
    error in ErlangError ->
      {:could_not_build, "heph could not be run: #{inspect(error.original)}"}
  end

  # heph prints the workload on its standard output and may warn on its error output
  # while succeeding, so the two are kept apart; the error output is what a failure
  # says.
  defp run_heph(arguments, number, approval) do
    errors = Path.join(System.tmp_dir!(), "heph-errors-#{System.unique_integer([:positive])}")

    try do
      case System.cmd("/bin/sh", [
             "-c",
             ~s(errors=$1; shift; exec "$@" 2>"$errors"),
             "sh",
             errors,
             heph() | arguments
           ]) do
        {output, 0} ->
          check_built(output, number, approval)

        {output, _} ->
          {:could_not_build, String.trim(String.trim(output) <> " " <> String.trim(read(errors)))}
      end
    after
      File.rm(errors)
    end
  end

  defp read(path) do
    case File.read(path) do
      {:ok, text} -> text
      {:error, _} -> ""
    end
  end

  defp check_built(output, number, approval) do
    expected = Integer.to_string(number)

    with {:ok,
          %{"ticket" => %{"number" => ^expected}, "approval" => %{"approvedBy" => login}} =
            workload} <-
           Jason.decode(output),
         true <- login == approval.login do
      {:ok, workload}
    else
      _ -> {:could_not_build, "heph did not build a workload for ticket #{number}"}
    end
  end

  # Where the ticket is judged after its pull request merges, the pull request must only refer
  # to it: a merge that closes the ticket ends it before the judgement, which only judges open
  # tickets, so the agent's claim would stand in place of the evidence.
  defp judged_afterwards(workload, repo) do
    if Evaluations.enabled?(repo), do: Map.put(workload, "judgedAfterMerge", true), else: workload
  end

  defp create(workload) do
    case Workloads.create(workload) do
      {:ok, record} -> {:ok, record}
      {:error, reason} -> {:could_not_build, "the workload could not be kept: #{reason}"}
    end
  end

  defp heph, do: Application.get_env(:control_plane, :intake, []) |> Keyword.get(:heph, "heph")
end
