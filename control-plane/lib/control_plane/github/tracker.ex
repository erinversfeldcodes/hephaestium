defmodule ControlPlane.GitHub.Tracker do
  @moduledoc """
  GitHub as a tracker: tickets are issues. This is where GitHub's own shapes (the
  `pull_request` key that tells an issue from a pull request, `labels[].name`,
  `sub_issues_summary`, the issue's events) are read, so nothing outside answers in them.
  """

  @behaviour ControlPlane.Tracker

  require Logger

  alias ControlPlane.{GitHub, Ticket, TicketComment, TicketEdit, TicketEvent}

  @edits_query """
  query($owner: String!, $name: String!, $number: Int!) {
    repository(owner: $owner, name: $name) {
      issue(number: $number) { userContentEdits(first: 100) { nodes { editedAt editor { login } } } }
    }
  }
  """

  @doc "The endpoint that lists a repository's open issues that carry a label."
  def ready_endpoint(repo, label),
    do: "repos/#{repo}/issues?labels=#{label}&state=open&per_page=100"

  @doc "The endpoint that lists an issue's events."
  def events_endpoint(repo, number), do: "repos/#{repo}/issues/#{number}/events?per_page=100"

  @impl true
  def capabilities,
    do: MapSet.new([:sub_tickets, :blocking_links, :label_actor_and_time, :act_as_person])

  @impl true
  def approved_tickets(project, label) do
    with {:ok, issues} <- GitHub.get_all(ready_endpoint(project, label)) do
      {:ok, for(issue <- issues, open_issue?(issue), do: ticket_of(issue))}
    end
  end

  defp open_issue?(issue), do: issue["state"] == "open" and is_nil(issue["pull_request"])

  @impl true
  def ticket(project, key) do
    with {:ok, issue} <- GitHub.get("repos/#{project}/issues/#{key}") do
      {:ok, ticket_of(issue)}
    end
  end

  @impl true
  def parent(project, key) do
    case GitHub.get("repos/#{project}/issues/#{key}/parent") do
      {:ok, %{"number" => _} = issue} -> {:ok, ticket_of(issue)}
      {:ok, _} -> {:ok, nil}
      {:error, message} -> if not_found?(message), do: {:ok, nil}, else: {:error, message}
    end
  end

  # A ticket that is nobody's sub-ticket has no parent: GitHub answers that as not found.
  defp not_found?(message),
    do: message =~ "Not Found" or message =~ ~s("status":"404") or message =~ "HTTP 404"

  @impl true
  def ticket_history(project, key, options \\ []) do
    result =
      case options[:pages] do
        :all -> GitHub.get_all(events_endpoint(project, key))
        _ -> GitHub.get(events_endpoint(project, key))
      end

    case result do
      {:ok, events} when is_list(events) -> {:ok, Enum.map(events, &event_of/1)}
      {:ok, _} -> {:ok, []}
      {:error, message} -> {:error, message}
    end
  end

  @impl true
  def ticket_edits(project, key) do
    [owner, name] = String.split(project, "/", parts: 2)

    with {:ok, body} <- GitHub.graphql(@edits_query, owner: owner, name: name, number: key),
         {:ok, nodes} <- edit_nodes(body) do
      {:ok,
       for node <- nodes do
         %TicketEdit{editor: get_in(node, ["editor", "login"]), at: time(node["editedAt"])}
       end}
    end
  end

  defp edit_nodes(%{
         "data" => %{"repository" => %{"issue" => %{"userContentEdits" => %{"nodes" => nodes}}}}
       })
       when is_list(nodes),
       do: {:ok, nodes}

  defp edit_nodes(other),
    do:
      {:error,
       "GitHub did not give the ticket's edits: #{inspect(other) |> String.slice(0, 200)}"}

  @impl true
  def open_epics(project) do
    case GitHub.get("repos/#{project}/issues?state=open&per_page=100") do
      {:ok, issues} when is_list(issues) ->
        {:ok, for(issue <- issues, is_nil(issue["pull_request"]), do: ticket_of(issue))}

      {:ok, _} ->
        {:ok, []}

      {:error, message} ->
        {:error, message}
    end
  end

  @impl true
  def sub_tickets(project, key) do
    case GitHub.get("repos/#{project}/issues/#{key}/sub_issues") do
      {:ok, children} when is_list(children) -> {:ok, Enum.map(children, &ticket_of/1)}
      {:ok, _} -> {:error, "GitHub did not answer with a list of sub-issues"}
      {:error, message} -> {:error, message}
    end
  end

  @impl true
  def comments(project, key, _options \\ []) do
    case GitHub.get("repos/#{project}/issues/#{key}/comments?per_page=100") do
      {:ok, comments} when is_list(comments) ->
        {:ok, for(c <- comments, do: %TicketComment{body: c["body"], at: time(c["created_at"])})}

      {:ok, _} ->
        {:ok, []}

      {:error, message} ->
        {:error, message}
    end
  end

  @impl true
  def create_ticket(project, title, body) do
    case GitHub.write("POST", "repos/#{project}/issues", ["title=" <> title, "body=" <> body]) do
      {:ok, %{"number" => number, "id" => id}} -> {:ok, %Ticket{key: number, id: id}}
      {:ok, _} -> {:error, "GitHub did not say which issue it made"}
      {:error, message} -> {:error, message}
    end
  end

  @impl true
  def link_sub_ticket(project, parent, %Ticket{id: id}) do
    ok(
      GitHub.write("POST", "repos/#{project}/issues/#{parent}/sub_issues", [
        {:typed, "sub_issue_id=#{id}"}
      ])
    )
  end

  @impl true
  def block_ticket(project, key, %Ticket{id: id}) do
    ok(
      GitHub.write("POST", "repos/#{project}/issues/#{key}/dependencies/blocked_by", [
        {:typed, "issue_id=#{id}"}
      ])
    )
  end

  @impl true
  def ensure_labels(project, labels) do
    with {:ok, existing} <- GitHub.get_all("repos/#{project}/labels?per_page=100") do
      present = for %{"name" => name} <- existing, into: MapSet.new(), do: name

      failures =
        for label <- labels, not MapSet.member?(present, label.name), reduce: [] do
          failures ->
            case create_label(project, label) do
              :ok -> failures
              {:error, message} -> failures ++ ["#{label.name}: #{message}"]
            end
        end

      if failures == [], do: :ok, else: {:error, Enum.join(failures, "; ")}
    end
  end

  # The only write to a repository's configuration the control plane makes, so it is said in
  # the log.
  defp create_label(project, label) do
    fields = ["name=#{label.name}", "color=#{label.colour}", "description=#{label.description}"]

    with {:ok, _} <- GitHub.write("POST", "repos/#{project}/labels", fields) do
      Logger.info("created the label #{label.name} in #{project}")
      :ok
    end
  end

  @impl true
  def add_label(project, key, label),
    do: logged("POST", issue(project, key) <> "/labels", ["labels[]=#{label}"])

  @impl true
  def remove_label(project, key, label),
    do: logged("DELETE", issue(project, key) <> "/labels/" <> URI.encode_www_form(label), [])

  @impl true
  def comment_on_ticket(project, key, body),
    do: logged("POST", issue(project, key) <> "/comments", ["body=" <> body])

  @impl true
  def close_ticket(project, key),
    do: logged("PATCH", issue(project, key), ["state=closed", "state_reason=completed"])

  @impl true
  def comment_as(project, key, token, body),
    do: as_person("POST", issue(project, key) <> "/comments", ["body=" <> body], token)

  @impl true
  def add_label_as(project, key, token, label),
    do: as_person("POST", issue(project, key) <> "/labels", ["labels[]=" <> label], token)

  # The request is made with the person's token, so GitHub decides what they may do and names
  # them as the actor. What GitHub says back is classified and not passed on.
  defp as_person(method, endpoint, fields, token) do
    case GitHub.write(method, endpoint, fields, token: token) do
      {:ok, _} -> :ok
      {:error, message} when is_binary(message) -> {:error, classify(message)}
    end
  end

  defp classify(message) do
    cond do
      message =~ "HTTP 401" -> :unauthorized
      message =~ ~r/HTTP (403|404|422)/ -> :refused
      true -> "GitHub could not be asked"
    end
  end

  defp issue(project, key), do: "repos/#{project}/issues/#{key}"

  # A write that is logged when it fails, because what it changes is what an operator looks
  # at the ticket for.
  defp logged(method, endpoint, fields) do
    case GitHub.write(method, endpoint, fields) do
      {:ok, _} ->
        :ok

      {:error, message} ->
        Logger.warning("could not #{method} #{endpoint}: #{message}")
        {:error, message}
    end
  end

  defp ok({:ok, _}), do: :ok
  defp ok({:error, message}), do: {:error, message}

  defp ticket_of(issue) do
    %Ticket{
      key: issue["number"],
      id: issue["id"],
      state: if(issue["state"] == "open", do: :open, else: :closed),
      labels: for(%{"name" => name} <- issue["labels"] || [], do: name),
      closed_at: time(issue["closed_at"]),
      progress: progress(issue["sub_issues_summary"])
    }
  end

  defp progress(%{"total" => total, "completed" => completed}) when is_integer(total),
    do: %{completed: completed, total: total}

  defp progress(_), do: nil

  @kinds %{
    "labeled" => :labeled,
    "unlabeled" => :unlabeled,
    "renamed" => :renamed,
    "closed" => :closed,
    "reopened" => :reopened
  }

  defp event_of(event) do
    %TicketEvent{
      kind: Map.get(@kinds, event["event"], :other),
      label: get_in(event, ["label", "name"]),
      actor: get_in(event, ["actor", "login"]),
      at: time(event["created_at"]),
      at_text: event["created_at"]
    }
  end

  defp time(text) when is_binary(text) do
    case DateTime.from_iso8601(text) do
      {:ok, at, _} -> at
      _ -> nil
    end
  end

  defp time(_), do: nil
end
