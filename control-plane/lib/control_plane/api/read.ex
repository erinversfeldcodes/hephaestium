defmodule ControlPlane.Api.Read do
  @moduledoc """
  What the API reads from the database, as maps ready to be written as JSON. Nothing here
  writes. The workload's own document and its record are not passed on whole: a workload is
  shown by what a person looks at, and its steps are read from the record on their own.
  """

  import Ecto.Query

  alias ControlPlane.{
    Account,
    DeviceTokens,
    Event,
    PullRequests,
    Repo,
    WaitingTickets,
    Workload,
    Workloads
  }

  @max_page 500

  @doc "The largest page of events one request may ask for."
  def max_page, do: @max_page

  @doc """
  The board's tickets, the most recently active first: each with its title, how many workloads
  it has had and the latest of them with its state, and its pull request (number, address, state and the
  state of its checks, as last recorded) or nil when it has none. Nothing here reads the code host. An approved ticket with no workload yet is
  among them with no `latest` and no workloads. A ticket that waits, with no workload or with a
  queued one, has `waiting`: since when, and for a queued workload whether no worker has
  claimed it, which is so when no executor of the ticket's approver is connected. A time the
  control plane does not hold is nil, and the age is unknown.
  """
  @spec tickets(non_neg_integer(), non_neg_integer()) :: [map()]
  def tickets(limit, offset) do
    rows = with_workloads(limit + offset)
    waiting = for w <- WaitingTickets.list(limit + offset), do: waiting_ticket(w)

    page =
      (rows ++ waiting)
      |> Enum.sort_by(& &1.sort_at, {:desc, DateTime})
      |> Enum.drop(offset)
      |> Enum.take(limit)

    connected =
      for %{waiting: %{approved_by: login, no_worker: :unknown}} <- page,
          is_binary(login),
          uniq: true,
          into: %{},
          do: {login, DeviceTokens.connected?(login)}

    pulls = PullRequests.for_tickets(for row <- page, do: {row.repository, row.number})

    Enum.map(page, fn row ->
      row
      |> Map.delete(:sort_at)
      |> Map.update!(:waiting, &worker_known(&1, connected))
      |> Map.put(:pull_request, pulls[{row.repository, row.number}])
    end)
  end

  defp worker_known(%{no_worker: :unknown, approved_by: login} = waiting, connected),
    do: %{waiting | no_worker: not Map.get(connected, login, false)}

  defp worker_known(waiting, _connected), do: waiting

  defp waiting_ticket(w) do
    %{
      repository: w.repository,
      number: w.ticket_number,
      title: w.title,
      workloads: 0,
      settled_at: nil,
      latest: nil,
      waiting: %{since: w.approved_at, approved_by: w.approved_by, no_worker: false},
      sort_at: w.approved_at
    }
  end

  defp with_workloads(limit) do
    latest =
      from w in Workload,
        distinct: [w.repository, w.ticket_number],
        order_by: [w.repository, w.ticket_number, desc: w.inserted_at, desc: w.id],
        select: %{
          id: w.id,
          repository: w.repository,
          number: w.ticket_number,
          state: w.state,
          inserted_at: w.inserted_at,
          title: fragment("?->'ticket'->>'title'", w.workload),
          kind:
            fragment(
              "(select k from jsonb_object_keys(?) k where k in ('revision', 'review', 'completeness', 'gap') limit 1)",
              w.workload
            ),
          settled_at: w.settled_at,
          approver: fragment("?->'approval'->>'approvedBy'", w.workload),
          workloads: fragment("count(*) over (partition by ?, ?)", w.repository, w.ticket_number)
        }

    Repo.all(
      from l in subquery(latest),
        order_by: [desc: l.inserted_at, asc: l.repository, asc: l.number],
        limit: ^limit
    )
    |> Enum.map(fn row ->
      %{
        repository: row.repository,
        number: row.number,
        title: row.title,
        workloads: row.workloads,
        settled_at: row.settled_at,
        latest: %{
          id: row.id,
          state: row.state,
          kind: row.kind || "implementation",
          at: row.inserted_at
        },
        waiting: queued(row),
        sort_at: utc(row.inserted_at)
      }
    end)
  end

  # A queued workload is waiting for a worker; whether any executor of its approver is
  # connected is read once for the page, after it is cut.
  defp queued(%{state: "queued", approver: approver, inserted_at: at}),
    do: %{since: utc(at), approved_by: approver, no_worker: :unknown}

  defp queued(_row), do: nil

  # Workloads keep their insert time without a zone; it is UTC.
  defp utc(%NaiveDateTime{} = at), do: DateTime.from_naive!(at, "Etc/UTC")
  defp utc(%DateTime{} = at), do: at

  @doc "A ticket's workloads, oldest first, or nil when it has none."
  @spec ticket(String.t(), integer()) :: map() | nil
  def ticket(repository, number) do
    case Workloads.for_ticket(repository, number) do
      [] ->
        nil

      [first | _] = workloads ->
        spends = ControlPlane.Spend.by_workload(repository, number)

        %{
          repository: repository,
          number: number,
          title: get_in(first.workload, ["ticket", "title"]),
          url: get_in(first.workload, ["ticket", "url"]),
          workloads: Enum.map(workloads, &with_spend(&1, spends)),
          pull_request: PullRequests.for_tickets([{repository, number}])[{repository, number}],
          spend: ControlPlane.Spend.ticket(repository, number)
        }
    end
  end

  # A workload that spent nothing carries zeros, so a reader never has to test for absence.
  defp with_spend(%Workload{} = w, spends) do
    spend = Map.get_lazy(spends, w.id, fn -> ControlPlane.Spend.figures([], w.state) end)
    w |> workload() |> Map.put(:spend, spend)
  end

  @doc "A workload as a person looks at it."
  @spec workload(%Workload{}) :: map()
  def workload(%Workload{} = w) do
    %{
      id: w.id,
      repository: w.repository,
      ticket_number: w.ticket_number,
      kind: kind(w.workload),
      state: w.state,
      reason: w.reason,
      parent_id: w.parent_id,
      approved_by: get_in(w.workload || %{}, ["approval", "approvedBy"]),
      started_at: w.started_at,
      finished_at: w.finished_at,
      settled_at: w.settled_at,
      created_at: w.inserted_at
    }
  end

  @doc """
  The gates a workload ran, each with its name and what it came to. Only the gate's own result
  is read: its outcome, why it did not run, the check it was left to, how long it took and its
  exit code. None of it is an agent's text.
  """
  @spec gates(%Workload{}) :: [map()]
  def gates(%Workload{record: %{"gates" => gates}}) when is_list(gates) do
    for %{"name" => name} = gate <- gates do
      result = if is_map(gate["result"]), do: gate["result"], else: %{}

      %{
        name: name,
        outcome: result["outcome"],
        reason: result["reason"],
        ci_check: result["ci_check"],
        duration_ms: result["duration_ms"],
        exit_code: result["exit_code"]
      }
    end
  end

  def gates(%Workload{}), do: []

  @doc """
  Every completeness verdict a ticket has had, oldest first. Each is the verdict as it passed
  the contract, with its requirements and the evidence for each, and when it was given. What
  an evaluation wrote is agent text, which is why this is read only behind the login.
  """
  @spec verdicts(String.t(), integer()) :: [map()]
  def verdicts(repository, number) do
    for %Workload{state: "completed", record: %{"completeness" => %{} = verdict}} = w <-
          Workloads.for_ticket(repository, number) do
      %{workload: w.id, at: w.finished_at || w.inserted_at, verdict: verdict}
    end
  end

  @doc """
  How much reviewing a ticket needed: the rounds its runs read their own change in before
  pushing, the reviews and revisions after, and for each lens the findings it made. A finding
  is still there when the newest review under its lens, which is the one that read the latest
  commit, makes it again, and is resolved when an earlier review made it and that one does not.
  A finding is the same finding when its title and where it is are the same.
  """
  @spec review_effort(String.t(), integer()) :: map()
  def review_effort(repository, number) do
    workloads = Workloads.for_ticket(repository, number)

    steps =
      for %Workload{record: %{"steps" => steps}} <- workloads,
          step when is_map(step) <- steps,
          do: step

    reviewed =
      for %Workload{workload: %{"review" => %{"lens" => lens}}, record: record} <- workloads,
          is_binary(lens),
          %{"findings" => findings} <- [(record || %{})["review"] || %{}],
          is_list(findings),
          do: {lens, findings}

    lenses =
      reviewed
      |> Enum.group_by(&elem(&1, 0), &elem(&1, 1))
      |> Enum.map(fn {lens, rounds} -> lens_effort(lens, rounds) end)
      |> Enum.sort_by(& &1.lens)

    %{
      local_rounds:
        steps
        |> Enum.filter(&(&1["kind"] in ["STEP_KIND_REVIEW", "KIND_REVIEW"]))
        |> Enum.map(&(&1["round"] || 0))
        |> Enum.uniq()
        |> length(),
      reviews: Enum.count(workloads, &(kind(&1.workload) == "review")),
      revisions: Enum.count(workloads, &(kind(&1.workload) == "revision")),
      lenses: lenses
    }
  end

  defp lens_effort(lens, rounds) do
    key = fn finding -> {finding["title"], finding["location"]} end
    latest = rounds |> List.last() |> Enum.map(key) |> MapSet.new()

    found =
      rounds
      |> List.flatten()
      |> Enum.uniq_by(key)
      |> Enum.map(fn finding ->
        %{
          title: finding["title"],
          severity: finding["severity"],
          blocking: finding["blocking"] == true,
          state: if(key.(finding) in latest, do: "remaining", else: "resolved")
        }
      end)

    %{
      lens: lens,
      reviews: length(rounds),
      found: length(found),
      resolved: Enum.count(found, &(&1.state == "resolved")),
      remaining: Enum.count(found, &(&1.state == "remaining")),
      findings: found
    }
  end

  @doc "What kind of run a workload's document asks for."
  @spec kind(map() | nil) :: String.t()
  def kind(document) do
    document = document || %{}
    Enum.find(~w(revision review completeness gap), "implementation", &Map.has_key?(document, &1))
  end

  @doc """
  A page of a workload's events after a sequence number, in order, with the cursor to ask for
  the next page (`nil` when this one was not full).
  """
  @spec events(Ecto.UUID.t(), non_neg_integer(), pos_integer()) :: %{
          events: [map()],
          next: integer() | nil
        }
  def events(workload_id, after_sequence, limit) do
    events = events_after(workload_id, after_sequence, limit)

    next = if length(events) == limit, do: List.last(events).sequence

    %{events: Enum.map(events, &event/1), next: next}
  end

  @doc "The events after a sequence number, up to a limit, as the schema rows."
  @spec events_after(Ecto.UUID.t(), non_neg_integer(), pos_integer()) :: [%Event{}]
  def events_after(workload_id, after_sequence, limit) do
    Repo.all(
      from e in Event,
        where: e.workload_id == ^workload_id and e.sequence > ^after_sequence,
        order_by: e.sequence,
        limit: ^limit
    )
  end

  @doc "An event as the API writes it."
  @spec event(%Event{}) :: map()
  def event(%Event{} = e),
    do: %{sequence: e.sequence, time: e.time, kind: e.kind, event: e.event}

  @doc """
  What an agent wrote in a run, which is read only behind the login: its last message as its
  account and, for a run that stopped to ask, its question. Both are cleaned of terminal escapes
  and are otherwise what the executor sent, whose redaction of the secrets it holds has already
  been applied. Either is nil when there is none.
  """
  @spec agent_text(%Workload{}) :: map()
  def agent_text(%Workload{} = w) do
    question =
      case (w.record || %{})["question"] do
        text when is_binary(text) and text != "" -> Account.clean(text) |> String.trim()
        _ -> nil
      end

    %{workload: w.id, account: Account.last_message(w.id), question: question}
  end

  @doc "The steps a workload's record lists, or none."
  @spec steps(%Workload{}) :: [map()]
  def steps(%Workload{record: %{"steps" => steps}}) when is_list(steps), do: steps
  def steps(%Workload{}), do: []

  @doc """
  The questions waiting for a person: workloads that ended blocked with a question, for a ticket
  that has had no workload since (a later workload is what an answer starts).
  """
  @spec questions() :: [map()]
  def questions do
    Repo.all(
      from w in Workload,
        as: :asked,
        where: w.state == "blocked",
        where: fragment("coalesce(?->>'question', '') <> ''", w.record),
        where:
          not exists(
            from l in Workload,
              where:
                l.repository == parent_as(:asked).repository and
                  l.ticket_number == parent_as(:asked).ticket_number and
                  l.inserted_at > parent_as(:asked).inserted_at,
              select: 1
          ),
        order_by: [desc: w.finished_at, desc: w.inserted_at]
    )
    |> Enum.map(fn w ->
      %{
        workload: w.id,
        repository: w.repository,
        ticket: w.ticket_number,
        title: get_in(w.workload, ["ticket", "title"]),
        question: w.record["question"],
        asked_at: w.finished_at
      }
    end)
  end
end
