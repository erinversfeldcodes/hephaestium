defmodule ControlPlane.Workloads do
  @moduledoc "Creating and reading workloads."

  import Ecto.Query

  require Logger

  alias ControlPlane.{Repo, Workload}

  @doc """
  Keep a workload, given as the contract's JSON mapping, queued.

  It must be a workload: an id that is a UUID, a repository with an owner and a
  name, and a ticket with a number (a number the mapping writes as a string).
  """
  @spec create(map(), keyword()) ::
          {:ok, %Workload{}} | {:error, :not_a_workload | :already_exists}
  def create(workload, options \\ [])

  def create(
        %{"id" => id, "repository" => %{"owner" => owner, "name" => name}} = workload,
        options
      )
      when is_binary(owner) and is_binary(name) do
    with {:ok, id} <- Ecto.UUID.cast(id),
         {:ok, number} <- ticket_number(workload) do
      %Workload{
        id: id,
        repository: "#{owner}/#{name}",
        ticket_number: number,
        state: "queued",
        workload: workload,
        parent_id: options[:parent],
        budget_base: options[:budget_base] || 0
      }
      |> Ecto.Changeset.change()
      |> Ecto.Changeset.unique_constraint(:id, name: :workloads_pkey)
      |> Repo.insert()
      |> case do
        {:ok, record} -> {:ok, record}
        {:error, _changeset} -> {:error, :already_exists}
      end
    else
      _ -> {:error, :not_a_workload}
    end
  end

  def create(_, _options), do: {:error, :not_a_workload}

  @doc """
  The workload of a ticket whose pull request is the one to look at: the latest that
  completed with a pull request in its record, which is not a review. Latest is by revision
  attempt, since workloads made in the same second cannot be told apart by when.
  """
  @spec pull_request_workload(String.t(), integer()) :: %Workload{} | nil
  def pull_request_workload(repository, number) do
    Repo.one(
      from w in Workload,
        where:
          w.repository == ^repository and w.ticket_number == ^number and w.state == "completed" and
            fragment("?->'review' IS NULL", w.workload) and
            fragment("?->'pullRequest'->>'url' IS NOT NULL", w.record),
        order_by: [
          desc: fragment("COALESCE((?->'revision'->>'attempt')::int, 0)", w.workload),
          desc: w.inserted_at
        ],
        limit: 1
    )
  end

  @doc """
  Resume a workload's pull request: it is unsettled again, so settling looks at the pull
  request as it is now, and the revisions it may have are counted from the attempt it had
  reached. `approval_at` is the approval this was done for.
  """
  @spec resume(%Workload{}, DateTime.t()) :: :ok
  def resume(%Workload{} = workload, approval_at) do
    Repo.update_all(from(w in Workload, where: w.id == ^workload.id),
      set: [
        settled_at: nil,
        resumed_for: approval_at,
        budget_base: get_in(workload.workload, ["revision", "attempt"]) || 0,
        updated_at: DateTime.utc_now()
      ]
    )

    :ok
  end

  @doc "The number of a workload's pull request, from the address its record holds."
  @spec pull_request_number(%Workload{}) :: {:ok, String.t()} | {:error, String.t()}
  def pull_request_number(%Workload{} = workload) do
    url = get_in(workload.record || %{}, ["pullRequest", "url"]) || ""

    case Regex.run(~r{^https://github\.com/([^/]+/[^/]+)/pull/(\d+)$}, url) do
      [_, repository, number] when repository == workload.repository -> {:ok, number}
      _ -> {:error, "the pull request #{inspect(url)} is not one of #{workload.repository}"}
    end
  end

  @doc """
  The branch the executor recorded when it opened the workload's pull request, or `nil` when
  the record holds none (a pull request opened before the branch was recorded).
  """
  @spec recorded_branch(%Workload{}) :: String.t() | nil
  def recorded_branch(%Workload{} = workload) do
    case get_in(workload.record || %{}, ["pullRequest", "headBranch"]) do
      branch when is_binary(branch) and branch != "" -> branch
      _ -> nil
    end
  end

  @legacy_prefix "heph/"

  @doc """
  Whether a change whose head is `head_ref` is the one this workload opened. The runtime's
  own change is the one whose head is the branch the executor recorded, whatever the branch
  is called: a person's change under a similar name is not it. A workload whose record holds
  no branch (opened before one was recorded) falls back on the name the runtime always gave
  then, which began `heph/`.
  """
  @spec runtime_branch?(%Workload{}, String.t() | nil) :: boolean()
  def runtime_branch?(%Workload{} = workload, head_ref) do
    head_ref = head_ref || ""

    case recorded_branch(workload) do
      nil -> String.starts_with?(head_ref, @legacy_prefix)
      recorded -> head_ref == recorded
    end
  end

  @doc """
  The settled workloads, in any repository, that opened a pull request, settled since the
  given time, the latest for each ticket.
  """
  @spec settled_pull_requests(DateTime.t()) :: [%Workload{}]
  def settled_pull_requests(since) do
    repositories =
      Repo.all(
        from w in Workload,
          where: not is_nil(w.settled_at) and w.updated_at >= ^since,
          distinct: true,
          select: w.repository
      )

    settled_with_pull_request(repositories, since)
  end

  @doc "Make a settled workload awaiting settlement again, so it is looked at on the next pass."
  @spec unsettle(String.t()) :: :ok
  def unsettle(id) do
    Repo.update_all(from(w in Workload, where: w.id == ^id),
      set: [settled_at: nil, updated_at: DateTime.utc_now()]
    )

    :ok
  end

  @doc """
  The settled workloads of these repositories that opened a pull request, settled since the
  given time, the latest for each ticket. A pull request that is merged after its ticket was
  settled is only seen by looking at these again.
  """
  @spec settled_with_pull_request([String.t()], DateTime.t()) :: [%Workload{}]
  def settled_with_pull_request(repositories, since) do
    Repo.all(
      from w in Workload,
        where:
          w.repository in ^repositories and not is_nil(w.settled_at) and w.updated_at >= ^since and
            fragment("?->'review' IS NULL", w.workload) and
            fragment("?->'completeness' IS NULL", w.workload) and
            fragment("?->'pullRequest'->>'url' IS NOT NULL", w.record),
        order_by: w.inserted_at
    )
    |> Enum.group_by(&{&1.repository, &1.ticket_number})
    |> Enum.map(fn {_ticket, workloads} -> List.last(workloads) end)
  end

  @doc "Every completeness evaluation of these repositories, oldest first."
  @spec evaluations([String.t()]) :: [%Workload{}]
  def evaluations(repositories) do
    Repo.all(
      from w in Workload,
        where:
          w.repository in ^repositories and
            fragment("?->'completeness' IS NOT NULL", w.workload),
        order_by: [w.inserted_at, w.started_at]
    )
  end

  @doc "Every step of the questioning of these repositories' tickets, oldest first."
  @spec steps([String.t()]) :: [%Workload{}]
  def steps(repositories) do
    Repo.all(
      from w in Workload,
        where: w.repository in ^repositories and fragment("?->'gap' IS NOT NULL", w.workload),
        order_by: [w.inserted_at, w.started_at]
    )
  end

  @doc """
  The workloads of a ticket, oldest first. Workloads made in the same instant are ordered by id,
  so the order is the same on every call rather than whatever the database happens to return.
  """
  @spec for_ticket(String.t(), integer()) :: [%Workload{}]
  def for_ticket(repository, number) do
    Repo.all(
      from w in Workload,
        where: w.repository == ^repository and w.ticket_number == ^number,
        order_by: [w.inserted_at, w.id]
    )
  end

  @doc """
  The workloads whose pull request is this change: the latest of each ticket, among those still
  awaiting settlement and those settled in the last fortnight. A webhook names a change, and this
  is how it finds the workload without asking the code host.
  """
  @spec for_change(String.t(), integer() | String.t()) :: [%Workload{}]
  def for_change(repository, number) do
    since = DateTime.add(DateTime.utc_now(), -14 * 86_400, :second)
    wanted = to_string(number)

    Repo.all(
      from w in Workload,
        where:
          w.repository == ^repository and w.state == "completed" and
            (is_nil(w.settled_at) or w.updated_at >= ^since) and
            fragment("?->'review' IS NULL", w.workload) and
            fragment("?->'completeness' IS NULL", w.workload) and
            fragment("?->'pullRequest'->>'url' IS NOT NULL", w.record),
        order_by: w.inserted_at
    )
    |> Enum.filter(&(pull_request_number(&1) == {:ok, wanted}))
    |> Enum.group_by(& &1.ticket_number)
    |> Enum.map(fn {_ticket, workloads} -> List.last(workloads) end)
  end

  @doc "A workload, or nil."
  @spec get(Ecto.UUID.t()) :: %Workload{} | nil
  def get(id), do: Repo.get(Workload, id)

  defp ticket_number(%{"ticket" => %{"number" => number}}) when is_integer(number),
    do: {:ok, number}

  defp ticket_number(%{"ticket" => %{"number" => number}}) when is_binary(number) do
    case Integer.parse(number) do
      {n, ""} when n > 0 -> {:ok, n}
      _ -> :error
    end
  end

  defp ticket_number(_), do: :error

  @terminal ~w(completed failed blocked timed_out cancelled not_needed)

  @doc "The states a workload does not leave."
  def terminal_states, do: @terminal

  @doc """
  Take a queued workload to run, once. A workload that is not queued (it is
  running, or has ended, or another job has it) is not taken again.
  """
  @spec claim(Ecto.UUID.t()) :: {:ok, %Workload{}} | :already_claimed | :unknown
  def claim(id) do
    {count, rows} =
      Repo.update_all(
        from(w in Workload, where: w.id == ^id and w.state == "queued", select: w),
        set: [state: "preparing", updated_at: DateTime.utc_now()]
      )

    cond do
      count == 1 -> {:ok, hd(rows)}
      get(id) == nil -> :unknown
      true -> :already_claimed
    end
  end

  @doc "Put a claimed workload back on the queue, because the executor could not be started."
  @spec release(Ecto.UUID.t()) :: :ok
  def release(id) do
    Repo.update_all(from(w in Workload, where: w.id == ^id and w.state == "preparing"),
      set: [state: "queued", updated_at: DateTime.utc_now()]
    )

    :ok
  end

  @doc """
  End a workload that has not ended as failed, with the reason. A workload that has
  already ended keeps the end it has.
  """
  @spec fail(Ecto.UUID.t(), String.t()) :: :ok
  def fail(id, reason) do
    now = DateTime.utc_now()

    Repo.update_all(
      from(w in Workload, where: w.id == ^id and w.state not in ^@terminal),
      set: [
        state: "failed",
        reason: ControlPlane.Clean.deep(reason),
        finished_at: now,
        updated_at: now
      ]
    )

    :ok
  end

  @doc """
  End a workload as not needed, with the reason: it had nothing left to do, which is no
  failure. It may replace a failure that was only a symptom of that (a revision that could not
  check out a branch that merged and was deleted), but never an end that was real.
  """
  @spec not_needed(Ecto.UUID.t(), String.t()) :: :ok
  def not_needed(id, reason) do
    now = DateTime.utc_now()

    Repo.update_all(
      from(w in Workload,
        where: w.id == ^id and (w.state not in ^@terminal or w.state in ["failed", "timed_out"])
      ),
      set: [
        state: "not_needed",
        reason: ControlPlane.Clean.deep(reason),
        finished_at: now,
        updated_at: now
      ]
    )

    :ok
  end

  @doc "Keep the run record the executor wrote."
  @spec keep_record(Ecto.UUID.t(), map()) :: :ok
  def keep_record(id, record) do
    Repo.update_all(from(w in Workload, where: w.id == ^id),
      set: [record: ControlPlane.Clean.deep(record), updated_at: DateTime.utc_now()]
    )

    write_ledger(id)
    :ok
  end

  # What the run spent goes in the ledger once its record is kept. The record is the source of
  # truth, so a ledger that could not be written does not fail keeping it: the backfill
  # repairs it.
  defp write_ledger(id) do
    ControlPlane.Ledger.record(get(id))
  rescue
    error ->
      Logger.error("could not write the ledger for workload #{id}: #{Exception.message(error)}")
  end

  @doc """
  The workloads that completed with a pull request and whose ticket has not been
  settled yet, oldest first.
  """
  def awaiting_settlement do
    Repo.all(
      from(w in Workload,
        where:
          w.state == "completed" and is_nil(w.settled_at) and
            fragment("?->'pullRequest'->>'url' IS NOT NULL", w.record),
        order_by: [asc: w.inserted_at]
      )
    )
  end

  @doc "Record that the ticket was settled."
  def settle(id) do
    Repo.update_all(from(w in Workload, where: w.id == ^id and is_nil(w.settled_at)),
      set: [settled_at: DateTime.utc_now(), updated_at: DateTime.utc_now()]
    )

    :ok
  end

  @doc """
  Claim the writing of what this workload found on its pull request. `:ok` for the one
  caller that gets it, `:already` for any other, so it is written once.
  """
  @spec claim_report(Ecto.UUID.t()) :: :ok | :already
  def claim_report(id) do
    now = DateTime.utc_now()

    case Repo.update_all(
           from(w in Workload, where: w.id == ^id and is_nil(w.reported_at)),
           set: [reported_at: now, updated_at: now]
         ) do
      {1, _} -> :ok
      _ -> :already
    end
  end

  @doc "Give the claim back, because the comment could not be written, so it is tried again."
  @spec release_report(Ecto.UUID.t()) :: :ok
  def release_report(id) do
    Repo.update_all(from(w in Workload, where: w.id == ^id), set: [reported_at: nil])
    :ok
  end

  @doc "Record that the check of this name on the workload's pull request was re-run."
  def count_rerun(id, name) do
    Repo.update_all(
      from(w in Workload,
        where: w.id == ^id,
        update: [
          set: [
            reruns:
              fragment(
                "jsonb_set(coalesce(?, '{}'::jsonb), ARRAY[?::text], to_jsonb(coalesce((? ->> ?)::int, 0) + 1))",
                w.reruns,
                ^name,
                w.reruns,
                ^name
              ),
            updated_at: ^DateTime.utc_now()
          ]
        ]
      ),
      []
    )

    :ok
  end

  @doc """
  The workloads a workload was revised from, oldest first, ending with it: the
  original, then each revision of its pull request.
  """
  @spec chain(%Workload{}) :: [%Workload{}]
  def chain(%Workload{} = workload), do: chain(workload, [workload])

  defp chain(%Workload{parent_id: nil}, acc), do: acc

  defp chain(%Workload{parent_id: parent_id}, acc) do
    case get(parent_id) do
      nil -> acc
      parent -> chain(parent, [parent | acc])
    end
  end

  @doc """
  Keep a review of a workload's pull request and put it on the queue, both or
  neither. The workload it reviews is not settled by this: a review reads, and what
  it says is for the pull request's own ticket to weigh.
  """
  @spec start_review(%Workload{}, map()) :: {:ok, %Workload{}} | {:error, term()}
  def start_review(%Workload{} = workload, review) do
    Repo.transaction(fn ->
      with {:ok, child} <- create(review, parent: workload.id),
           {:ok, _job} <- ControlPlane.Runner.enqueue(child.id) do
        child
      else
        {:error, reason} -> Repo.rollback(reason)
      end
    end)
  end

  @doc """
  Hand a workload's pull request on to a revision: keep the revision, put it on the
  queue, and mark the workload settled, all or none, so that a revision is started
  once and a workload that was handed on is never judged again.
  """
  @spec hand_on(%Workload{}, map()) :: {:ok, %Workload{}} | {:error, term()}
  def hand_on(%Workload{} = workload, revision) do
    Repo.transaction(fn ->
      with {:ok, child} <-
             create(revision, parent: workload.id, budget_base: workload.budget_base),
           {:ok, _job} <- ControlPlane.Runner.enqueue(child.id) do
        settle(workload.id)
        child
      else
        {:error, reason} -> Repo.rollback(reason)
      end
    end)
  end
end
