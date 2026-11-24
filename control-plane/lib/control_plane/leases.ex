defmodule ControlPlane.Leases do
  @moduledoc """
  A workload held by an executor on someone's machine: claimed as a lease, kept alive by what
  the executor sends, and put right when it is not.

  A claim records which token holds the workload and when the lease ends. Every event and every
  heartbeat moves the end on, so the lease lapses after the configured silence. Whether a
  lease is good is decided by the clock at the moment of each request, under a lock on the
  workload's row, and never by whether a sweep has noticed yet: an executor whose lease has
  lapsed is refused even in the seconds before it is cleaned up. The same lock is held to put
  a workload back, so an event and a lapse cannot both win.

  A lost workload (its lease lapsed), or one its executor gives back because its operator
  stopped it, is put back on the queue only while nothing could have been
  published. What the lost run said stays kept, because the stream of events is append-only and
  is the evidence of what happened, and the run that follows goes on from the next number: the stream has no event that says delivery began, and the workload has been put
  back fewer times than the link allows. Otherwise it is failed as a lost run is on the local
  runner, because running it again could push twice.

  Events and the record are kept by the code the local runner keeps them with, so a run that
  came over the network is stored exactly as one that came over a pipe.
  """

  import Ecto.Query

  require Logger

  alias ControlPlane.{
    Events,
    Event,
    ExecutorLink,
    Repo,
    RevisionNeeded,
    Tickets,
    Workload,
    WorkloadLease,
    Workloads
  }

  alias ControlPlane.DeviceToken

  @type held_error :: :not_held

  # The states in which a workload holds its ticket's branch.
  @in_flight ["preparing", "running", "gating"]

  # The key of the lock that serialises claims.
  @claim_lock 331_001

  # --- claiming -------------------------------------------------------------------------

  @doc """
  Give the token's person a queued workload their login approved, as a lease, or `:none`: a
  review before any other kind, so an open pull request is not held up by new work, and the
  oldest first within each. A workload that changes a ticket's branch is not given while another
  that does is under way for the same ticket: two would push to the one branch, and the second to
  push is refused. It stays queued until the first ends. A workload that is not queued, or that has a lease, is never given, and one that is
  being claimed at this moment is skipped and not waited for, so two executors never both get
  one.
  """
  @spec claim(%DeviceToken{}) :: {:ok, %Workload{}} | :none
  def claim(%DeviceToken{} = token) do
    reap()
    approver = String.downcase(token.login)

    claimed =
      Repo.transaction(fn ->
        # One claim at a time, so that two executors asking at once cannot each see the ticket
        # free and each take a workload for it.
        Repo.query!("SELECT pg_advisory_xact_lock($1)", [@claim_lock])

        candidate =
          Repo.one(
            from w in Workload,
              as: :w,
              left_join: l in WorkloadLease,
              on: l.workload_id == w.id and l.active,
              where: w.state == "queued" and is_nil(l.workload_id),
              where:
                not is_nil(fragment("?->'review'", w.workload)) or
                  not exists(
                    from o in Workload,
                      where:
                        o.repository == parent_as(:w).repository and
                          o.ticket_number == parent_as(:w).ticket_number and
                          o.id != parent_as(:w).id and
                          o.state in @in_flight and
                          is_nil(fragment("?->'review'", o.workload)),
                      select: 1
                  ),
              where: fragment("lower(?->'approval'->>'approvedBy')", w.workload) == ^approver,
              order_by: [
                asc: fragment("CASE WHEN ?->'review' IS NULL THEN 1 ELSE 0 END", w.workload),
                asc: w.inserted_at,
                asc: w.id
              ],
              limit: 1,
              lock: "FOR UPDATE OF w0 SKIP LOCKED"
          )

        case candidate do
          nil -> Repo.rollback(:none)
          workload -> take(workload, token)
        end
      end)

    case claimed do
      {:ok, workload} ->
        case RevisionNeeded.check(workload) do
          :needed ->
            # The ticket stops looking approved, as when the local runner starts an executor.
            Tickets.started(workload)
            {:ok, workload}

          {:not_needed, reason} ->
            # Its pull request merged or closed while it waited: it is not given, and the
            # next one is looked for.
            release(workload.id)
            Workloads.not_needed(workload.id, reason)
            claim(token)
        end

      {:error, :none} ->
        :none
    end
  end

  defp take(workload, token) do
    now = now()

    Repo.insert!(
      %WorkloadLease{
        workload_id: workload.id,
        token_id: token.id,
        active: true,
        expires_at: lease_end(now)
      },
      on_conflict: [
        set: [token_id: token.id, active: true, expires_at: lease_end(now), updated_at: now]
      ],
      conflict_target: :workload_id
    )

    {1, [taken]} =
      Repo.update_all(
        from(w in Workload, where: w.id == ^workload.id, select: w),
        set: [state: "preparing", updated_at: now]
      )

    taken
  end

  @doc "The number the next event of a workload's stream must have."
  @spec next_sequence(Ecto.UUID.t()) :: pos_integer()
  def next_sequence(workload_id) do
    (Repo.one(from e in Event, where: e.workload_id == ^workload_id, select: max(e.sequence)) || 0) +
      1
  end

  # --- what a holder sends --------------------------------------------------------------

  @doc """
  Keep events from the executor that holds the workload, in order, by the rules of the stream.

  An event the control plane has already kept, identical, is accepted again without being
  kept twice, because a request that the network cut may be sent again and a stream that
  failed its executor for that would fail for the network's sake. Any other repeat is a
  broken rule. An event that breaks a rule ends the workload as failed, as it does locally,
  and what came before it in the batch stays kept.
  """
  @spec ingest(%DeviceToken{}, Ecto.UUID.t(), [map()]) ::
          {:ok, non_neg_integer(), DateTime.t()}
          | {:error, held_error()}
          | {:error, {:rule, atom(), non_neg_integer()}}
  def ingest(token, workload_id, events) when is_list(events) do
    events
    |> Enum.reduce_while({:ok, 0, nil}, fn event, {:ok, count, _} ->
      case ingest_one(token, workload_id, event) do
        {:ok, expires_at} -> {:cont, {:ok, count + 1, expires_at}}
        {:error, :not_held} -> {:halt, {:error, :not_held}}
        {:error, {:rule, rule}} -> {:halt, {:error, {:rule, rule, count}}}
      end
    end)
    |> case do
      {:ok, 0, _} ->
        with {:ok, at} <- heartbeat(token, workload_id), do: {:ok, 0, at}

      other ->
        other
    end
  end

  defp ingest_one(token, workload_id, event) do
    result =
      Repo.transaction(fn ->
        lock!(workload_id)
        hold!(workload_id, token)

        case kept?(workload_id, event) do
          true ->
            :ok

          false ->
            case Events.append(workload_id, event) do
              {:ok, _} -> :ok
              {:error, rule} -> Repo.rollback({:rule, rule})
            end
        end

        extend(workload_id)
      end)

    case result do
      {:ok, expires_at} ->
        {:ok, expires_at}

      {:error, :not_held} ->
        {:error, :not_held}

      {:error, {:rule, rule}} ->
        end_for_rule(token, workload_id, rule)
    end
  end

  # An event that is already kept, whole. Compared as it would be stored.
  defp kept?(workload_id, event) when is_map(event) do
    with {sequence, ""} <- Integer.parse(to_string(event["sequence"] || "")),
         %Event{event: stored} <-
           Repo.get_by(Event, workload_id: workload_id, sequence: sequence) do
      stored == ControlPlane.Clean.deep(event)
    else
      _ -> false
    end
  end

  defp kept?(_workload_id, _event), do: false

  # The stream broke a rule, so the workload ends as failed and its lease is let go, in one step
  # that a lapse cannot come between.
  defp end_for_rule(token, workload_id, rule) do
    ended =
      Repo.transaction(fn ->
        lock!(workload_id)
        hold!(workload_id, token)
        release(workload_id)
        Workloads.fail(workload_id, rule_reason(rule))
      end)

    case ended do
      {:ok, _} ->
        Tickets.ended(Workloads.get(workload_id))
        {:error, {:rule, rule}}

      {:error, :not_held} ->
        {:error, :not_held}
    end
  end

  defp rule_reason(:not_an_event),
    do: "the executor wrote a line that is not an event (not_an_event)"

  defp rule_reason(rule), do: "the stream broke a rule (#{rule})"

  @doc "Move the end of the lease on: the executor is there. Returns when it now ends."
  @spec heartbeat(%DeviceToken{}, Ecto.UUID.t()) :: {:ok, DateTime.t()} | {:error, held_error()}
  def heartbeat(token, workload_id) do
    Repo.transaction(fn ->
      lock!(workload_id)
      hold!(workload_id, token)
      extend(workload_id)
    end)
  end

  @doc """
  The executor is over and sends its record: it is kept as the local runner keeps one (cleaned,
  and its spend written to the ledger), a workload that did not say how it ended is failed, the
  lease is let go and the ticket is told.

  The lease is let go before anything is written, so a sweep can never put back a workload whose
  record is being kept.
  """
  @spec finish(%DeviceToken{}, Ecto.UUID.t(), map()) :: :ok | {:error, held_error()}
  def finish(token, workload_id, record) when is_map(record) do
    released =
      Repo.transaction(fn ->
        lock!(workload_id)
        hold!(workload_id, token)
        release(workload_id)
      end)

    case released do
      {:ok, _} ->
        Workloads.keep_record(workload_id, record)

        Workloads.fail(
          workload_id,
          "the executor ended without saying how it ended"
        )

        Tickets.ended(Workloads.get(workload_id))
        :ok

      {:error, :not_held} ->
        {:error, :not_held}
    end
  end

  @doc """
  The executor's operator stopped it, so it gives the workload back instead of ending it. The
  lapse rules apply at once: the workload goes back on the queue, or is failed when delivery
  had begun (it may have pushed) or it has been put back as often as the link allows. Nothing
  about the ticket changed, so the stop is not a verdict on the work.
  """
  @spec give_back(%DeviceToken{}, Ecto.UUID.t()) ::
          {:ok, :requeued | :failed | :ended} | {:error, held_error()}
  def give_back(token, workload_id) do
    handled =
      Repo.transaction(fn ->
        lock!(workload_id)
        hold!(workload_id, token)
        decide(workload_id, Repo.get(WorkloadLease, workload_id))
      end)

    case handled do
      {:ok, :requeued} ->
        {:ok, :requeued}

      {:ok, {:failed, _}} ->
        Tickets.ended(Workloads.get(workload_id))
        {:ok, :failed}

      {:ok, :ended} ->
        Tickets.ended(Workloads.get(workload_id))
        {:ok, :ended}

      {:error, :not_held} ->
        {:error, :not_held}
    end
  end

  # --- the lease ------------------------------------------------------------------------

  defp lock!(workload_id) do
    Repo.one(from w in Workload, where: w.id == ^workload_id, lock: "FOR UPDATE") ||
      Repo.rollback(:not_held)
  end

  # The lease is good: held by this token, not let go, and not past its end by the clock now.
  defp hold!(workload_id, token) do
    now = now()

    case Repo.get(WorkloadLease, workload_id) do
      %WorkloadLease{active: true, token_id: token_id, expires_at: expires_at}
      when token_id == token.id ->
        if DateTime.compare(expires_at, now) == :gt, do: :ok, else: Repo.rollback(:not_held)

      _ ->
        Repo.rollback(:not_held)
    end
  end

  defp extend(workload_id) do
    at = lease_end(now())

    Repo.update_all(from(l in WorkloadLease, where: l.workload_id == ^workload_id),
      set: [expires_at: at, updated_at: now()]
    )

    at
  end

  defp release(workload_id) do
    Repo.update_all(from(l in WorkloadLease, where: l.workload_id == ^workload_id),
      set: [active: false, updated_at: now()]
    )

    :ok
  end

  # --- what a lapse becomes -------------------------------------------------------------

  @doc """
  Look after every lease that has lapsed: put its workload back on the queue, or fail it.
  Returns what became of each. Safe to run from any number of places at once.
  """
  @spec reap() :: [%{workload_id: Ecto.UUID.t(), result: :requeued | :failed | :ended}]
  def reap do
    lapsed =
      Repo.all(
        from l in WorkloadLease,
          where: l.active and l.expires_at <= ^now(),
          select: l.workload_id
      )

    for workload_id <- lapsed, outcome = lose(workload_id), do: outcome
  end

  defp lose(workload_id) do
    handled =
      Repo.transaction(fn ->
        lock!(workload_id)

        # Looked at again under the lock: an event may have moved the end on since the sweep read it.
        lease = Repo.get(WorkloadLease, workload_id)

        if lease && lease.active && DateTime.compare(lease.expires_at, now()) != :gt do
          decide(workload_id, lease)
        else
          Repo.rollback(:not_lapsed)
        end
      end)

    case handled do
      {:ok, :requeued} ->
        %{workload_id: workload_id, result: :requeued}

      {:ok, {:failed, _}} ->
        Tickets.ended(Workloads.get(workload_id))
        %{workload_id: workload_id, result: :failed}

      {:ok, :ended} ->
        Tickets.ended(Workloads.get(workload_id))
        %{workload_id: workload_id, result: :ended}

      {:error, _} ->
        nil
    end
  end

  defp decide(workload_id, lease) do
    workload = Repo.get!(Workload, workload_id)
    release(workload_id)

    cond do
      # It said how it ended; only its record was lost with the executor.
      workload.state in Workloads.terminal_states() ->
        :ended

      delivery_began?(workload_id) ->
        reason =
          "the executor was lost after it began to deliver, so the run is not started again: it may have pushed"

        Workloads.fail(workload_id, reason)
        {:failed, reason}

      lease.requeues >= ExecutorLink.config(:max_requeues) ->
        reason =
          "the executor was lost, and the workload has been put back #{lease.requeues} times already"

        Workloads.fail(workload_id, reason)
        {:failed, reason}

      true ->
        requeue(workload, lease)
        :requeued
    end
  end

  defp delivery_began?(workload_id),
    do:
      Repo.exists?(
        from e in Event, where: e.workload_id == ^workload_id and e.kind == "deliveryBegan"
      )

  # Back on the queue as a workload that has not begun. The events of the lost run stay, and
  # the executor that takes it next is told where the numbering goes on.
  defp requeue(workload, lease) do
    Repo.update_all(from(l in WorkloadLease, where: l.workload_id == ^workload.id),
      set: [requeues: lease.requeues + 1]
    )

    Repo.update_all(from(w in Workload, where: w.id == ^workload.id),
      set: [state: "queued", started_at: nil, reason: nil, updated_at: now()]
    )
  end

  defp lease_end(from), do: DateTime.add(from, ExecutorLink.config(:lease_seconds), :second)

  defp now, do: DateTime.utc_now()
end
