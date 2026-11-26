defmodule ControlPlane.PullRequests do
  @moduledoc """
  The record of each pull request the control plane knows of: its address, whether it is open,
  merged or closed, and how its checks stand. A webhook (or the hourly poll) is the cue to look:
  the code host is read for the current state and that is what is stored, so a late, repeated
  or out-of-order delivery leaves the record right and a repeated one leaves one row. Reads of
  the record (`for_tickets/1`) touch only the database.

  A pull request is known when a workload of the repository opened it, or a row already holds
  it. A delivery naming any other is not looked at.
  """

  import Ecto.Query

  require Logger

  alias ControlPlane.{Check, Change, CodeHost, PullRequest, Repo, Workload, Workloads}

  @good ["success", "skipped", "neutral"]
  @stale_after 3600
  @recent_days 14

  @doc """
  How a pull request's checks stand: `none` when there are none, `failing` when any ended as
  anything but success, skipped or neutral (a failure is not waited on while others run),
  `pending` while any still runs, otherwise `passing`.
  """
  @spec checks_state([Check.t()]) :: String.t()
  def checks_state([]), do: "none"

  def checks_state(checks) do
    cond do
      Enum.any?(checks, &(&1.state == :completed and &1.conclusion not in @good)) -> "failing"
      Enum.any?(checks, &(&1.state != :completed)) -> "pending"
      true -> "passing"
    end
  end

  @doc """
  Read one pull request from the code host and store it, if it is known. `:unknown` when it is
  not, `:unchanged` when the code host's answer matched the record, `:recorded` otherwise.
  """
  @spec refresh(String.t(), integer() | String.t()) ::
          :recorded | :unchanged | :unknown | {:error, String.t()}
  def refresh(repository, number) do
    number = to_integer(number)

    if known?(repository, number), do: read_and_store(repository, number), else: :unknown
  end

  @doc "`refresh/2` for each open pull request of the repository that is at this commit."
  @spec refresh_head(String.t(), String.t() | nil) :: [term()]
  def refresh_head(_repository, nil), do: []

  def refresh_head(repository, sha) do
    numbers =
      Repo.all(
        from p in PullRequest,
          where: p.repository == ^repository and p.head_sha == ^sha and p.state == "open",
          select: p.number
      )

    for number <- numbers, do: refresh(repository, number)
  end

  @doc """
  The poll: store each pull request a recent workload opened that has no row, and look again
  at each open one whose row the code host has not confirmed for an hour. This is what repairs a
  delivery that never came. Returns the results.
  """
  @spec refresh_due(DateTime.t()) :: [term()]
  def refresh_due(now) do
    stale_before = DateTime.add(now, -@stale_after, :second)

    due =
      Repo.all(
        from p in PullRequest,
          where: p.state == "open" and p.refreshed_at < ^stale_before,
          select: {p.repository, p.number}
      )

    for {repository, number} <- Enum.uniq(unrecorded() ++ due) do
      read_and_store(repository, number)
    end
  end

  defp unrecorded do
    held = MapSet.new(Repo.all(from p in PullRequest, select: {p.repository, p.number}))

    for pair <- opened_by_workloads(), not MapSet.member?(held, pair), do: pair
  end

  defp known?(repository, number) do
    Repo.exists?(
      from p in PullRequest, where: p.repository == ^repository and p.number == ^number
    ) or
      {repository, number} in opened_by_workloads(repository)
  end

  defp opened_by_workloads(repository \\ nil) do
    since = DateTime.add(DateTime.utc_now(), -@recent_days * 86_400, :second)

    query =
      from w in Workload,
        where:
          w.state == "completed" and w.updated_at >= ^since and
            fragment("?->'review' IS NULL", w.workload) and
            fragment("?->'completeness' IS NULL", w.workload) and
            fragment("?->'pullRequest'->>'url' IS NOT NULL", w.record),
        select: %{repository: w.repository, url: fragment("?->'pullRequest'->>'url'", w.record)}

    query = if repository, do: where(query, [w], w.repository == ^repository), else: query

    for workload <- Repo.all(query),
        {:ok, number} <- [Workloads.pull_request_number(workload.repository, workload.url)],
        uniq: true,
        do: {workload.repository, String.to_integer(number)}
  end

  defp read_and_store(repository, number) do
    with {:ok, %Change{} = change} <- CodeHost.change(repository, number),
         {:ok, state} <- state_of(change),
         {:ok, checks} <- checks_of(repository, number, change, state) do
      store(repository, number, change, state, checks)
    else
      :skip -> :unchanged
      {:error, message} -> {:error, to_string(message)}
    end
  rescue
    exception ->
      Logger.error(
        "could not record pull request #{number} of #{repository}: " <>
          Exception.message(exception)
      )

      {:error, Exception.message(exception)}
  end

  defp state_of(%Change{merged: true}), do: {:ok, "merged"}
  defp state_of(%Change{state: :open}), do: {:ok, "open"}
  defp state_of(%Change{state: :closed}), do: {:ok, "closed"}
  defp state_of(%Change{}), do: :skip

  # Only an open pull request's checks are read: a merged or closed one keeps what it had.
  defp checks_of(repository, _number, %Change{head_sha: sha}, "open") when is_binary(sha) do
    with {:ok, checks} <- CodeHost.checks(repository, sha), do: {:ok, checks_state(checks)}
  end

  defp checks_of(repository, number, _change, _state) do
    case Repo.get_by(PullRequest, repository: repository, number: number) do
      %PullRequest{checks: checks} -> {:ok, checks}
      nil -> {:ok, "none"}
    end
  end

  # One statement, so that two deliveries for the same pull request make one row, and
  # `changed_at` moves only when the state or the checks differ from what was held.
  defp store(repository, number, change, state, checks) do
    now = DateTime.utc_now()
    url = change.url || ""
    sha = change.head_sha

    %{rows: [[existed, same]]} =
      Repo.query!(
        """
        WITH held AS (
          SELECT state, checks FROM pull_requests WHERE repository = $1 AND number = $2
        ), written AS (
          INSERT INTO pull_requests AS p
            (repository, number, url, state, checks, head_sha, changed_at, refreshed_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $7)
          ON CONFLICT (repository, number) DO UPDATE SET
            url = EXCLUDED.url,
            state = EXCLUDED.state,
            checks = EXCLUDED.checks,
            head_sha = EXCLUDED.head_sha,
            refreshed_at = EXCLUDED.refreshed_at,
            changed_at = CASE WHEN p.state = EXCLUDED.state AND p.checks = EXCLUDED.checks
                              THEN p.changed_at ELSE EXCLUDED.changed_at END
          RETURNING 1
        )
        SELECT (SELECT count(*) FROM held), (SELECT count(*) FROM held WHERE state = $4 AND checks = $5)
        """,
        [repository, number, url, state, checks, sha, now]
      )

    if existed == 1 and same == 1, do: :unchanged, else: :recorded
  end

  @doc """
  The pull requests of the tickets given as `{repository, ticket_number}`: for each ticket
  that has one, `%{number:, url:, state:, checks:}`, taken from the newest workload that
  opened one. Reads the database only.
  """
  @spec for_tickets([{String.t(), integer()}]) :: %{{String.t(), integer()} => map()}
  def for_tickets([]), do: %{}

  def for_tickets(tickets) do
    repositories = tickets |> Enum.map(&elem(&1, 0)) |> Enum.uniq()
    numbers = tickets |> Enum.map(&elem(&1, 1)) |> Enum.uniq()
    wanted = MapSet.new(tickets)

    opened =
      Repo.all(
        from w in Workload,
          where:
            w.repository in ^repositories and w.ticket_number in ^numbers and
              fragment("?->'pullRequest'->>'url' IS NOT NULL", w.record),
          order_by: [asc: w.inserted_at, asc: w.id],
          select: %{
            repository: w.repository,
            ticket_number: w.ticket_number,
            url: fragment("?->'pullRequest'->>'url'", w.record)
          }
      )

    # The newest workload that opened one wins, so a later pull request replaces an earlier.
    numbered =
      for w <- opened,
          MapSet.member?(wanted, {w.repository, w.ticket_number}),
          {:ok, number} <- [Workloads.pull_request_number(w.repository, w.url)],
          reduce: %{} do
        acc -> Map.put(acc, {w.repository, w.ticket_number}, String.to_integer(number))
      end

    rows =
      Repo.all(
        from p in PullRequest,
          where: p.repository in ^repositories and p.number in ^Enum.uniq(Map.values(numbered))
      )
      |> Map.new(&{{&1.repository, &1.number}, &1})

    for {{repository, _} = ticket, number} <- numbered,
        %PullRequest{} = row <- [rows[{repository, number}]],
        into: %{},
        do: {ticket, %{number: row.number, url: row.url, state: row.state, checks: row.checks}}
  end

  defp to_integer(number) when is_integer(number), do: number
  defp to_integer(number) when is_binary(number), do: String.to_integer(number)
end
