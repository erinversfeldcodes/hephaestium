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

  alias ControlPlane.{GitHub, Runner, Workloads}

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

  @type result :: :started | {:skipped, term()}

  @doc "Check every approved ticket of a repository, and start a workload for each that stands."
  @spec run(String.t()) :: [%{ticket: integer(), result: result()}] | {:error, String.t()}
  def run(repo) do
    case GitHub.get_all(ready_endpoint(repo)) do
      {:ok, issues} ->
        for issue <- issues, open_issue?(issue) do
          %{ticket: issue["number"], result: consider(repo, issue["number"])}
        end

      {:error, message} ->
        {:error, message}
    end
  end

  defp open_issue?(issue), do: issue["state"] == "open" and is_nil(issue["pull_request"])

  defp consider(repo, number) do
    with {:ok, events} <- fetch(GitHub.get_all(events_endpoint(repo, number))),
         {:ok, approval} <- approval(events),
         {:ok, true} <- writer(repo, approval.login, :approver_cannot_write),
         :ok <- not_started(repo, number, approval.at),
         :ok <- not_edited_since(repo, number, events, approval),
         {:ok, workload} <- build(repo, number, approval),
         {:ok, record} <- create(workload),
         {:ok, _job} <- Runner.enqueue(record.id) do
      :started
    else
      {:ok, false, reason} -> {:skipped, reason}
      {:skip, reason} -> {:skipped, reason}
      {:github_error, message} -> {:skipped, {:github_error, message}}
      {:could_not_build, message} -> {:skipped, {:could_not_build, message}}
      other -> {:skipped, {:unexpected, inspect(other)}}
    end
  end

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
        end
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

  defp build(repo, number, approval) do
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

    case System.cmd(heph(), arguments, stderr_to_stdout: true) do
      {output, 0} -> check_built(output, number, approval)
      {output, _} -> {:could_not_build, String.trim(output)}
    end
  rescue
    error in ErlangError ->
      {:could_not_build, "heph could not be run: #{inspect(error.original)}"}
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
