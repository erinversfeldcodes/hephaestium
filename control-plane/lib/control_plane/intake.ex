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

  alias ControlPlane.{Evaluations, GitHub, Programs, Runner, Tickets, Workloads}

  @label "heph:ready"
  @writers ~w(admin maintain write)

  def ready_endpoint(repo), do: "repos/#{repo}/issues?labels=#{@label}&state=open&per_page=100"
  def events_endpoint(repo, number), do: "repos/#{repo}/issues/#{number}/events?per_page=100"
  def permission_endpoint(repo, login), do: "repos/#{repo}/collaborators/#{login}/permission"

  @edits_query """
  query($owner: String!, $name: String!, $number: Int!) {
    repository(owner: $owner, name: $name) {
      issue(number: $number) { userContentEdits(first: 100) { nodes { editedAt editor { login } } } }
    }
  }
  """

  @type result :: :started | :resumed | {:evaluating, String.t()} | {:skipped, term()}

  @doc "Check every approved ticket of a repository, and start a workload for each that stands."
  @spec run(String.t()) :: [%{ticket: integer(), result: result()}] | {:error, String.t()}
  def run(repo) do
    case GitHub.get_all(ready_endpoint(repo)) do
      {:ok, issues} ->
        for issue <- issues, open_issue?(issue) do
          %{ticket: issue["number"], result: consider(repo, issue)}
        end

      {:error, message} ->
        {:error, message}
    end
  end

  defp open_issue?(issue), do: issue["state"] == "open" and is_nil(issue["pull_request"])

  defp consider(repo, issue) do
    number = issue["number"]

    with {:ok, events} <- fetch(GitHub.get_all(events_endpoint(repo, number))),
         {:ok, approval} <- approval(events),
         {:ok, true} <- writer(repo, approval.login, :approver_cannot_write),
         :ok <- not_started(repo, number, approval.at),
         :ok <- not_edited_since(repo, number, events, approval),
         {:ok, plan} <- plan(repo, number) do
      act(plan, repo, issue, approval)
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
             {:ok, pull} <- fetch(GitHub.get("repos/#{repo}/pulls/#{pull_number}")) do
          case resumable(repo, pull) do
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
  defp resumable(repo, %{"merged" => true}),
    do: if(Evaluations.enabled?(repo), do: :evaluate, else: {:skip, :pull_request_merged})

  defp resumable(_repo, %{"state" => "closed"}), do: {:skip, :pull_request_closed}

  defp resumable(repo, %{"state" => "open", "head" => head}) do
    if String.starts_with?(head["ref"] || "", "heph/") and
         get_in(head, ["repo", "full_name"]) == repo,
       do: :ok,
       else: {:skip, :not_a_runtime_pull_request}
  end

  defp resumable(_repo, _pull), do: {:skip, :not_a_runtime_pull_request}

  defp act(:build, repo, issue, approval) do
    number = issue["number"]

    with {:ok, workload} <- build(repo, number, approval),
         :ok <- unblocked(workload),
         {:ok, record} <- create(workload),
         {:ok, _job} <- Runner.enqueue(record.id) do
      :started
    else
      {:blocked, numbers} -> {:skipped, {:blocked_by, numbers}}
      {:github_error, message} -> {:skipped, {:github_error, message}}
      {:could_not_build, message} -> {:skipped, {:could_not_build, message}}
      other -> {:skipped, {:unexpected, inspect(other)}}
    end
  end

  defp act({:resume, workload, pull}, _repo, issue, approval) do
    :ok = Workloads.resume(workload, approval.at)
    labels = for %{"name" => name} <- issue["labels"] || [], do: name
    _ = Tickets.resumed(workload, labels, approval.login, pull["html_url"])
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
    |> Enum.filter(&(&1["event"] == "labeled" and get_in(&1, ["label", "name"]) == @label))
    |> Enum.flat_map(fn event ->
      with login when is_binary(login) <- get_in(event, ["actor", "login"]),
           {:ok, at, _} <- DateTime.from_iso8601(event["created_at"] || "") do
        [%{login: login, at: at, text: event["created_at"]}]
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
    case GitHub.get(permission_endpoint(repo, login)) do
      {:ok, %{"permission" => permission}} when permission in @writers ->
        {:ok, true}

      {:ok, _} ->
        {:ok, false, reason}

      {:error, message} ->
        if message =~ "404", do: {:ok, false, reason}, else: {:github_error, message}
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
          event["event"] == "renamed",
          after?(event["created_at"], approval.at),
          do: get_in(event, ["actor", "login"])

    [owner, name] = String.split(repo, "/", parts: 2)

    with {:ok, body} <-
           fetch(GitHub.graphql(@edits_query, owner: owner, name: name, number: number)),
         {:ok, nodes} <- edit_nodes(body) do
      editors =
        for node <- nodes,
            after?(node["editedAt"], approval.at),
            do: get_in(node, ["editor", "login"])

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

  defp edit_nodes(%{
         "data" => %{"repository" => %{"issue" => %{"userContentEdits" => %{"nodes" => nodes}}}}
       })
       when is_list(nodes),
       do: {:ok, nodes}

  defp edit_nodes(other),
    do:
      {:github_error,
       "GitHub did not give the ticket's edits: #{inspect(other) |> String.slice(0, 200)}"}

  defp after?(text, at) when is_binary(text) do
    case DateTime.from_iso8601(text) do
      {:ok, time, _} -> DateTime.compare(time, at) == :gt
      _ -> true
    end
  end

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

  defp create(workload) do
    case Workloads.create(workload) do
      {:ok, record} -> {:ok, record}
      {:error, reason} -> {:could_not_build, "the workload could not be kept: #{reason}"}
    end
  end

  defp heph, do: Application.get_env(:control_plane, :intake, []) |> Keyword.get(:heph, "heph")
end
