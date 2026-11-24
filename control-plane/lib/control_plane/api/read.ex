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
    Finding,
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

  Each carries `runs` (its workloads, oldest first, with kind, state, lens and time: what the
  board draws a track from), `spend_usd`, and `needs`: `%{kind: "question", text: ...}` for a
  question waiting, `%{kind: "failure", step: ..., runs: n}` for a ticket whose latest run
  failed and nothing has restarted (the gate it stopped at, or the run's kind, and how many runs
  in a row stopped there), or nil when it needs no person.
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

    keys = for row <- page, do: {row.repository, row.number}
    pulls = PullRequests.for_tickets(keys)
    runs = runs_of(keys)
    spent = spent_usd(keys)
    asked = for q <- questions(), into: %{}, do: {{q.repository, q.ticket}, q.question}

    Enum.map(page, fn row ->
      key = {row.repository, row.number}
      row_runs = Map.get(runs, key, [])

      row
      |> Map.delete(:sort_at)
      |> Map.update!(:waiting, &worker_known(&1, connected))
      |> Map.put(:pull_request, pulls[key])
      |> Map.put(:runs, row_runs)
      |> Map.put(:spend_usd, Map.get(spent, key, 0.0))
      |> Map.put(:needs, needs(row, asked[key], row_runs))
    end)
  end

  # What a person is asked for on a ticket: the question a run stopped to ask, or the step a run
  # failed at when nothing has started a run since. A settled ticket, and one that needs nothing,
  # carry nil.
  defp needs(%{settled_at: settled}, _question, _runs) when not is_nil(settled), do: nil

  defp needs(_row, question, runs) do
    case {question, Enum.reverse(runs)} do
      {text, _} when is_binary(text) and text != "" ->
        %{kind: "question", text: text |> Account.clean() |> String.trim()}

      {_, [%{state: state} = latest | earlier]} when state in ["failed", "timed_out"] ->
        same =
          Enum.take_while(
            earlier,
            &(&1.state in ["failed", "timed_out"] and &1.failed_gate == latest.failed_gate and
                &1.kind == latest.kind)
          )

        %{kind: "failure", step: latest.failed_gate || latest.kind, runs: 1 + length(same)}

      _ ->
        nil
    end
  end

  # Each ticket's workloads, oldest first, as far as the board needs to place a ticket on its
  # track: their kind, state, lens and when they were made, and the gate a failed one stopped at
  # (nil when it stopped at none).
  defp runs_of([]), do: %{}

  defp runs_of(keys) do
    repositories = keys |> Enum.map(&elem(&1, 0)) |> Enum.uniq()
    numbers = keys |> Enum.map(&elem(&1, 1)) |> Enum.uniq()
    wanted = MapSet.new(keys)

    Repo.all(
      from w in Workload,
        where: w.repository in ^repositories and w.ticket_number in ^numbers,
        order_by: [asc: w.inserted_at, asc: w.id],
        select: %{
          repository: w.repository,
          number: w.ticket_number,
          state: w.state,
          at: w.inserted_at,
          document: w.workload,
          gates: fragment("?->'gates'", w.record)
        }
    )
    |> Enum.filter(&MapSet.member?(wanted, {&1.repository, &1.number}))
    |> Enum.group_by(&{&1.repository, &1.number}, fn w ->
      kind = kind(w.document)

      %{
        kind: kind,
        state: w.state,
        lens: get_in(w.document || %{}, ["review", "lens"]),
        at: utc(w.at),
        failed_gate: failed_gate(w.gates)
      }
    end)
  end

  defp failed_gate(gates) when is_list(gates) do
    Enum.find_value(gates, fn
      %{"name" => name, "result" => %{"outcome" => outcome}}
      when outcome in ["failed", "timed out", "timed_out"] ->
        name

      _ ->
        nil
    end)
  end

  defp failed_gate(_), do: nil

  defp spent_usd([]), do: %{}

  defp spent_usd(keys) do
    repositories = keys |> Enum.map(&elem(&1, 0)) |> Enum.uniq()
    numbers = keys |> Enum.map(&elem(&1, 1)) |> Enum.uniq()
    wanted = MapSet.new(keys)

    Repo.all(
      from e in ControlPlane.SpendEntry,
        where: e.repository in ^repositories and e.ticket_number in ^numbers and e.unit == "usd",
        group_by: [e.repository, e.ticket_number],
        select: {{e.repository, e.ticket_number}, coalesce(sum(e.amount), 0.0)}
    )
    |> Enum.filter(fn {key, _} -> MapSet.member?(wanted, key) end)
    |> Map.new(fn {key, usd} -> {key, usd * 1.0} end)
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
      created_at: w.inserted_at,
      provenance: provenance(w)
    }
  end

  @doc """
  What ran a workload: the control plane that made it and the executor that ran it. A part the
  workload was kept without is `%{known: false}`; it is never filled in from what is set today,
  because a run can only be compared with another by what it ran under.
  """
  @spec provenance(%Workload{}) :: map()
  def provenance(%Workload{} = w) do
    %{control_plane: control_plane(w.provenance), executor: executor(w.record)}
  end

  defp control_plane(%{"controlPlaneCommit" => commit} = kept) do
    %{
      known: true,
      commit: commit,
      athenaeum_ref: kept["athenaeumRef"]
    }
  end

  defp control_plane(%{} = kept) when map_size(kept) > 0,
    do: %{known: true, commit: nil, athenaeum_ref: kept["athenaeumRef"]}

  defp control_plane(_), do: %{known: false}

  defp executor(%{"provenance" => %{} = kept}) do
    %{
      known: true,
      athenaeum_tag: blank(kept["athenaeumTag"]),
      athenaeum_commit: blank(kept["athenaeumCommit"]),
      commit: blank(kept["executorCommit"]),
      skills: kept["skills"] || [],
      lenses: kept["lenses"] || []
    }
  end

  defp executor(_), do: %{known: false}

  defp blank(""), do: nil
  defp blank(value), do: value

  @doc """
  The gates a workload ran, each with its name and what it came to: its outcome, why it did not
  run, the check it was left to, how long it took and its exit code, and the last lines it
  printed (at most 40) with how many lines came before them. The output can carry private paths,
  so this is read only behind the login. A record that holds more than 40 lines is cut again here.
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
      |> Map.merge(output(gate))
    end
  end

  def gates(%Workload{}), do: []

  @output_lines 40

  # The last lines a gate printed, and how many lines were left out: those the executor left out
  # and any this cuts.
  defp output(gate) do
    text = if is_binary(gate["output"]), do: gate["output"], else: ""

    left_out =
      if is_integer(gate["omittedLines"]) and gate["omittedLines"] > 0,
        do: gate["omittedLines"],
        else: 0

    lines = String.split(text, "\n")
    cut = max(length(lines) - @output_lines, 0)

    %{
      output: lines |> Enum.drop(cut) |> Enum.join("\n"),
      omitted_lines: left_out + cut
    }
  end

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

  Rounds are the reviews under a lens, counted from 1 in the order they were made. A finding
  carries the round that first raised it, every round that made it, the round that fixed it
  (the first review after its last appearance, or nil while it is still there), and the URL of
  the comment that review was posted as (nil when it was not posted). What the record does not
  have is nil.
  """
  @spec review_effort(String.t(), integer()) :: map()
  def review_effort(repository, number) do
    workloads = Workloads.for_ticket(repository, number)

    steps =
      for %Workload{record: %{"steps" => steps}} <- workloads,
          step when is_map(step) <- steps,
          do: step

    reviewed =
      for %Workload{workload: %{"review" => %{"lens" => lens}}, record: record} = workload <-
            workloads,
          is_binary(lens),
          %{"findings" => findings} <- [(record || %{})["review"] || %{}],
          is_list(findings),
          do: {lens, %{findings: for(%{} = f <- findings, do: f), url: workload.report_url}}

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
    numbered = rounds |> Enum.with_index(1) |> Enum.map(fn {round, n} -> {n, round} end)
    last = length(rounds)

    found =
      numbered
      |> Enum.flat_map(fn {n, round} -> Enum.map(round.findings, &{n, round, &1}) end)
      |> Enum.group_by(fn {_, _, finding} -> key.(finding) end)
      |> Enum.map(fn {_key, [{raised, round, finding} | _] = seen} ->
        appeared = seen |> Enum.map(&elem(&1, 0)) |> Enum.uniq()
        latest = List.last(appeared)

        %{
          order: {raised, Enum.find_index(round.findings, &(key.(&1) == key.(finding)))},
          title: finding["title"],
          severity: finding["severity"],
          blocking: finding["blocking"] == true,
          state: if(latest == last, do: "remaining", else: "resolved"),
          location: Finding.place(finding["location"]),
          evidence: text(finding["evidence"]),
          revision: text(finding["revision"]),
          raised_round: raised,
          rounds: appeared,
          fixed_round: if(latest == last, do: nil, else: latest + 1),
          comment_url: round.url
        }
      end)
      |> Enum.sort_by(& &1.order)
      |> Enum.map(&Map.delete(&1, :order))

    %{
      lens: lens,
      reviews: length(rounds),
      found: length(found),
      resolved: Enum.count(found, &(&1.state == "resolved")),
      remaining: Enum.count(found, &(&1.state == "remaining")),
      findings: found
    }
  end

  defp text(value) when is_binary(value) and value != "", do: value
  defp text(_), do: nil

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
