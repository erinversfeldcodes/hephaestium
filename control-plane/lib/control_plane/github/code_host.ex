defmodule ControlPlane.GitHub.CodeHost do
  @moduledoc """
  GitHub as a code host: changes are pull requests, checks are check runs. This is where
  GitHub's own shapes (`mergeable_state`, `head.sha`, `base.ref`, the check runs, the
  ruleset, the contents API) are read, so nothing outside answers in them. The review App
  that approves a change is GitHub's alone and is used only by `approve/4`.
  """

  @behaviour ControlPlane.CodeHost

  alias ControlPlane.{Change, ChangeReview, Check, GitHub, ReviewApp}

  @impl true
  def capabilities, do: MapSet.new([:approval, :check_rerun, :check_annotations])

  @impl true
  def change(repository, number) do
    with {:ok, pull} <- GitHub.get("repos/#{repository}/pulls/#{number}") do
      {:ok, change_of(pull, number)}
    end
  end

  defp change_of(pull, number) do
    head = pull["head"] || %{}

    %Change{
      number: number,
      url: pull["html_url"],
      state: state(pull["state"]),
      merged: pull["merged"] == true,
      head_sha: head["sha"],
      head_ref: head["ref"],
      head_repository: get_in(head, ["repo", "full_name"]),
      base_ref: get_in(pull, ["base", "ref"]),
      mergeability: mergeability(pull["mergeable_state"])
    }
  end

  defp state("open"), do: :open
  defp state("closed"), do: :closed
  defp state(_), do: :other

  defp mergeability(state) when state in [nil, "unknown"], do: :unknown
  defp mergeability("clean"), do: :clean
  defp mergeability("behind"), do: :behind
  defp mergeability("blocked"), do: :blocked
  defp mergeability("dirty"), do: :conflicted
  defp mergeability("draft"), do: :draft
  defp mergeability(other), do: {:other, to_string(other)}

  @impl true
  def change_files(repository, number, options \\ []) do
    result =
      case options[:pages] do
        :all -> GitHub.get_all("repos/#{repository}/pulls/#{number}/files")
        _ -> GitHub.get("repos/#{repository}/pulls/#{number}/files?per_page=100")
      end

    with {:ok, files} <- result do
      {:ok, for(%{"filename" => name} <- List.wrap(files), do: name)}
    end
  end

  @impl true
  def compare(repository, from, to) do
    with {:ok, compare} <- GitHub.get("repos/#{repository}/compare/#{from}...#{to}") do
      commits =
        for %{"sha" => sha, "commit" => %{"message" => message}} <- compare["commits"] || [],
            do: %{sha: sha, message: message}

      {:ok, %{commits: commits, files: for(file <- compare["files"] || [], do: file["filename"])}}
    end
  end

  @impl true
  def comment_on_change(repository, number, body) do
    case GitHub.write("POST", "repos/#{repository}/issues/#{number}/comments", ["body=" <> body]) do
      {:ok, %{"html_url" => url}} when is_binary(url) -> {:ok, url}
      {:ok, _} -> :ok
      {:error, message} -> {:error, message}
    end
  end

  @impl true
  def merge(repository, number, commit) do
    case GitHub.write("PUT", "repos/#{repository}/pulls/#{number}/merge", [
           "merge_method=squash",
           "sha=#{commit}"
         ]) do
      {:ok, _} ->
        :ok

      {:error, message} ->
        # GitHub saying the change cannot be merged (not allowed, conflicting, the head
        # moved) is an answer. Anything else may pass.
        if Regex.match?(~r/HTTP (405|409|422)/, message),
          do: {:refused, message},
          else: {:error, message}
    end
  end

  @impl true
  def reviews(repository, number) do
    with {:ok, reviews} <- GitHub.get_all("repos/#{repository}/pulls/#{number}/reviews") do
      {:ok,
       for review <- reviews do
         %ChangeReview{
           author: get_in(review, ["user", "login"]),
           state: review["state"],
           commit: review["commit_id"]
         }
       end}
    end
  end

  # Approved once for a head commit however often it is asked: the review App's own earlier
  # approval of this commit stands, and an approval of another commit is not this one's.
  @impl true
  def approve(repository, number, commit, body) do
    case ReviewApp.token(repository) do
      :disabled ->
        :disabled

      {:error, message} ->
        {:error, message}

      {:ok, %{token: token, login: login}} ->
        with {:ok, reviews} <- reviews(repository, number) do
          if Enum.any?(reviews, &approved_by?(&1, login, commit)),
            do: :approved,
            else: submit(repository, number, commit, body, token)
        end
    end
  end

  defp approved_by?(review, login, commit),
    do: review.author == login and review.state == "APPROVED" and review.commit == commit

  defp submit(repository, number, commit, body, token) do
    case GitHub.write(
           "POST",
           "repos/#{repository}/pulls/#{number}/reviews",
           ["event=APPROVE", "commit_id=#{commit}", "body=#{body}"],
           token: token
         ) do
      {:ok, _} -> :submitted
      {:error, message} -> {:error, message}
    end
  end

  @impl true
  def required_checks(repository, branch) do
    with {:ok, rules} <- GitHub.get("repos/#{repository}/rules/branches/#{branch}") do
      contexts =
        for %{
              "type" => "required_status_checks",
              "parameters" => %{"required_status_checks" => checks}
            } <-
              rules,
            %{"context" => context} <- checks,
            uniq: true,
            do: context

      {:ok, contexts}
    end
  end

  @impl true
  def checks(repository, commit) do
    with {:ok, runs} <-
           GitHub.get("repos/#{repository}/commits/#{commit}/check-runs?per_page=100") do
      {:ok, for(run <- runs["check_runs"] || [], do: check_of(run))}
    end
  end

  defp check_of(run) do
    output = run["output"] || %{}

    %Check{
      id: run["id"],
      name: run["name"],
      state: if(run["status"] == "completed", do: :completed, else: :running),
      conclusion: run["conclusion"],
      title: output["title"],
      summary: output["summary"],
      suite: (run["check_suite"] || %{})["id"]
    }
  end

  @impl true
  def check_log(repository, id), do: GitHub.text("repos/#{repository}/actions/jobs/#{id}/logs")

  @impl true
  def check_annotations(repository, id) do
    case GitHub.get("repos/#{repository}/check-runs/#{id}/annotations") do
      {:ok, annotations} when is_list(annotations) ->
        {:ok, for(%{"message" => message} <- annotations, is_binary(message), do: message)}

      {:ok, _} ->
        {:ok, []}

      {:error, message} ->
        {:error, message}
    end
  end

  @impl true
  def rerun_check(repository, id),
    do: ok(GitHub.write("POST", "repos/#{repository}/actions/jobs/#{id}/rerun", []))

  @impl true
  def file(repository, path, ref),
    do: GitHub.raw("repos/#{repository}/contents/#{path}?ref=#{ref}")

  @impl true
  def directory(repository, path, ref) do
    case GitHub.get("repos/#{repository}/contents/#{path}?ref=#{ref}") do
      {:ok, entries} when is_list(entries) ->
        {:ok, for(%{"type" => type, "name" => name} <- entries, do: %{name: name, type: type})}

      {:ok, _} ->
        {:error, :not_a_list}

      {:error, message} ->
        {:error, message}
    end
  end

  @doc "The endpoint that says what a login may do in a repository."
  def permission_endpoint(repo, login), do: "repos/#{repo}/collaborators/#{login}/permission"

  @impl true
  def permission(repository, login) do
    case GitHub.get(permission_endpoint(repository, login)) do
      {:ok, %{"permission" => "admin"}} -> {:ok, :admin}
      {:ok, %{"permission" => "maintain"}} -> {:ok, :maintain}
      {:ok, %{"permission" => "write"}} -> {:ok, :write}
      {:ok, %{"permission" => "read"}} -> {:ok, :read}
      {:ok, _} -> {:ok, :none}
      {:error, message} -> if message =~ "404", do: {:ok, :none}, else: {:error, message}
    end
  end

  defp ok({:ok, _}), do: :ok
  defp ok({:error, message}), do: {:error, message}
end
