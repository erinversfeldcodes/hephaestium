defmodule ControlPlane.GitHub.Webhook do
  @moduledoc """
  GitHub's webhooks: how a delivery is signed (HMAC-SHA256 of the body in
  `X-Hub-Signature-256`), how it is named (`X-GitHub-Delivery`) and what each of the
  events subscribed to means in the control plane's own words. This is where GitHub's payloads
  are read, so nothing outside answers in them, and an event keeps none of the payload's text.
  """

  @behaviour ControlPlane.Webhooks.Source

  alias ControlPlane.TicketLabels
  alias ControlPlane.Webhooks.Event

  @impl true
  def verified?("", _headers, _body), do: false

  def verified?(secret, headers, body) when is_binary(secret) do
    case header(headers, "x-hub-signature-256") do
      "sha256=" <> presented when byte_size(presented) == 64 ->
        expected = :crypto.mac(:hmac, :sha256, secret, body) |> Base.encode16(case: :lower)
        Plug.Crypto.secure_compare(presented, expected)

      _ ->
        false
    end
  end

  @impl true
  def delivery_id(headers) do
    case header(headers, "x-github-delivery") do
      id when is_binary(id) and id != "" -> {:ok, id}
      _ -> :error
    end
  end

  @impl true
  def normalise(headers, body, now) do
    with {:ok, id} <- delivery_id(headers),
         name when is_binary(name) <- header(headers, "x-github-event") do
      if name in ~w(issues issue_comment pull_request pull_request_review check_run check_suite status push) do
        with {:ok, %{} = payload} <- Jason.decode(body),
             {:ok, repository} <- repository(payload) do
          base = %Event{
            source: "github",
            repository: repository,
            actor: get_in(payload, ["sender", "login"]),
            at: now,
            delivery_id: id
          }

          event(name, payload["action"], payload, base)
        else
          _ -> :error
        end
      else
        :ignore
      end
    else
      _ -> :error
    end
  end

  defp header(headers, name) do
    case List.keyfind(headers, name, 0) do
      {_, value} -> value
      nil -> nil
    end
  end

  defp repository(%{"repository" => %{"full_name" => name}}) when is_binary(name) and name != "",
    do: {:ok, name}

  defp repository(_), do: :error

  # Issues are tickets.
  defp event("issues", action, %{"issue" => %{"number" => number}} = payload, base)
       when is_integer(number) do
    ticket = %{base | number: number}
    label = get_in(payload, ["label", "name"])
    ready = TicketLabels.ready()

    case {action, label} do
      {"labeled", ^ready} -> {:ok, %{ticket | kind: :ticket_approved}}
      {"unlabeled", ^ready} -> {:ok, %{ticket | kind: :ticket_edited, cause: :unapproved}}
      {"edited", _} -> {:ok, %{ticket | kind: :ticket_edited, cause: :edited}}
      {"closed", _} -> {:ok, %{ticket | kind: :ticket_closed}}
      {"reopened", _} -> {:ok, %{ticket | kind: :ticket_reopened}}
      _ -> :ignore
    end
  end

  # GitHub sends a comment on a ticket and one on a change as the same event.
  defp event("issue_comment", "created", %{"issue" => %{"number" => number} = issue}, base)
       when is_integer(number) do
    on = if Map.has_key?(issue, "pull_request"), do: :change, else: :ticket
    {:ok, %{base | kind: :comment_added, number: number, on: on}}
  end

  defp event("issue_comment", _action, %{"issue" => %{"number" => number}}, _base)
       when is_integer(number),
       do: :ignore

  defp event("pull_request", action, %{"pull_request" => %{"number" => number} = pull}, base)
       when is_integer(number) do
    change = %{base | number: number, head: get_in(pull, ["head", "sha"])}

    case {action, pull["merged"]} do
      {"closed", true} -> {:ok, %{change | kind: :change_merged}}
      {"closed", _} -> {:ok, %{change | kind: :change_closed}}
      {"opened", _} -> {:ok, %{change | kind: :change_updated, cause: :opened}}
      {"reopened", _} -> {:ok, %{change | kind: :change_updated, cause: :opened}}
      {"ready_for_review", _} -> {:ok, %{change | kind: :change_updated, cause: :opened}}
      {"synchronize", _} -> {:ok, %{change | kind: :change_updated, cause: :synchronize}}
      {"edited", _} -> {:ok, %{change | kind: :change_updated, cause: :edited}}
      _ -> :ignore
    end
  end

  defp event(
         "pull_request_review",
         action,
         %{"pull_request" => %{"number" => number} = pull},
         base
       )
       when is_integer(number) do
    if action in ["submitted", "dismissed"],
      do:
        {:ok,
         %{
           base
           | kind: :change_updated,
             cause: :review,
             number: number,
             head: get_in(pull, ["head", "sha"])
         }},
      else: :ignore
  end

  # A run that ended is what settling waits for; one that began or is running only moves the
  # state of the change's checks.
  defp event("check_run", action, %{"check_run" => %{"head_sha" => sha} = run}, base)
       when is_binary(sha) do
    kind = if action == "completed", do: :checks_completed, else: :checks_changed
    {:ok, %{base | kind: kind, number: first_number(run["pull_requests"]), head: sha}}
  end

  defp event("check_run", _action, %{"check_run" => %{}}, _base), do: :ignore

  defp event("check_suite", action, %{"check_suite" => %{"head_sha" => sha} = suite}, base)
       when is_binary(sha) do
    kind = if action == "completed", do: :checks_completed, else: :checks_changed
    {:ok, %{base | kind: kind, number: first_number(suite["pull_requests"]), head: sha}}
  end

  defp event("check_suite", _action, %{"check_suite" => %{}}, _base), do: :ignore

  # A commit status names a commit and no change.
  defp event("status", _action, %{"sha" => sha} = payload, base) when is_binary(sha) do
    kind = if payload["state"] == "pending", do: :checks_changed, else: :checks_completed
    {:ok, %{base | kind: kind, number: nil, head: sha}}
  end

  # Only a push to the default branch moves what a pull request is based on; its own branch
  # arrives as the pull request being synchronised.
  defp event("push", _action, %{"ref" => ref} = payload, base) when is_binary(ref) do
    default = get_in(payload, ["repository", "default_branch"])

    if is_binary(default) and ref == "refs/heads/" <> default and payload["deleted"] != true,
      do: {:ok, %{base | kind: :base_moved, head: payload["after"]}},
      else: :ignore
  end

  defp event(_name, _action, _payload, _base), do: :error

  defp first_number([%{"number" => number} | _]) when is_integer(number), do: number
  defp first_number(_), do: nil
end
