defmodule ControlPlane.Api.Read do
  @moduledoc """
  What the API reads from the database, as maps ready to be written as JSON. Nothing here
  writes. The workload's own document and its record are not passed on whole: a workload is
  shown by what a person looks at, and its steps are read from the record on their own.
  """

  import Ecto.Query

  alias ControlPlane.{Event, Repo, Workload, Workloads}

  @max_page 500

  @doc "The largest page of events one request may ask for."
  def max_page, do: @max_page

  @doc """
  The tickets that have workloads, the most recently active first: each with its title, how
  many workloads it has had and the latest of them with its state.
  """
  @spec tickets(non_neg_integer(), non_neg_integer()) :: [map()]
  def tickets(limit, offset) do
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
          workloads: fragment("count(*) over (partition by ?, ?)", w.repository, w.ticket_number)
        }

    Repo.all(
      from l in subquery(latest),
        order_by: [desc: l.inserted_at, asc: l.repository, asc: l.number],
        limit: ^limit,
        offset: ^offset
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
        }
      }
    end)
  end

  @doc "A ticket's workloads, oldest first, or nil when it has none."
  @spec ticket(String.t(), integer()) :: map() | nil
  def ticket(repository, number) do
    case Workloads.for_ticket(repository, number) do
      [] ->
        nil

      [first | _] = workloads ->
        %{
          repository: repository,
          number: number,
          title: get_in(first.workload, ["ticket", "title"]),
          url: get_in(first.workload, ["ticket", "url"]),
          workloads: Enum.map(workloads, &workload/1),
          spend: ControlPlane.Spend.ticket(repository, number)
        }
    end
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
