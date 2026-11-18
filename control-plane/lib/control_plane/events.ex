defmodule ControlPlane.Events do
  @moduledoc """
  A workload's stream of events, and the rules it must keep.

  An event is kept only if it is the next in its workload's stream. The
  executor guarantees that; this is where a stream from another process is read,
  so it is checked and not trusted. An event that breaks a rule is refused with
  the rule, and what that means for the workload is for the caller to decide.
  """

  import Ecto.Query

  alias ControlPlane.{Event, Repo, Workload}

  @outcomes %{
    "OUTCOME_COMPLETED" => "completed",
    "OUTCOME_FAILED" => "failed",
    "OUTCOME_BLOCKED" => "blocked",
    "OUTCOME_TIMED_OUT" => "timed_out",
    "OUTCOME_CANCELLED" => "cancelled"
  }

  @envelope ["workloadId", "sequence", "time"]

  @type refusal ::
          :not_an_event
          | :wrong_workload
          | :unknown_workload
          | :after_finished
          | :repeated_sequence
          | :gap_in_sequence
          | :time_goes_back

  @doc "The events of a workload, in order."
  @spec list(Ecto.UUID.t()) :: [%Event{}]
  def list(workload_id) do
    Repo.all(from e in Event, where: e.workload_id == ^workload_id, order_by: e.sequence)
  end

  @doc "Keep the next event of a workload's stream, given as the contract's JSON mapping."
  @spec append(Ecto.UUID.t(), map()) :: {:ok, %Event{}} | {:error, refusal()}
  def append(workload_id, event) do
    with {:ok, parsed} <- parse(event),
         :ok <- belongs_to(parsed, workload_id) do
      Repo.transaction(fn -> keep(workload_id, event, parsed) end)
      |> case do
        {:ok, kept} -> {:ok, kept}
        {:error, refusal} -> {:error, refusal}
      end
    end
  end

  defp keep(workload_id, event, parsed) do
    # The workload's row is locked for the length of the append, so that two
    # writers cannot both read the same last event.
    workload =
      Repo.one(from w in Workload, where: w.id == ^workload_id, lock: "FOR UPDATE") ||
        Repo.rollback(:unknown_workload)

    last =
      Repo.one(
        from e in Event,
          where: e.workload_id == ^workload_id,
          order_by: [desc: e.sequence],
          limit: 1
      )

    check_rules!(last, parsed)

    kept =
      Repo.insert!(%Event{
        workload_id: workload_id,
        sequence: parsed.sequence,
        time: parsed.time,
        kind: parsed.kind,
        event: event
      })

    workload |> advance(parsed) |> Repo.update!()
    kept
  end

  defp check_rules!(last, parsed) do
    expected = if last, do: last.sequence + 1, else: 1

    cond do
      last && last.kind == "finished" -> Repo.rollback(:after_finished)
      parsed.sequence < expected -> Repo.rollback(:repeated_sequence)
      parsed.sequence > expected -> Repo.rollback(:gap_in_sequence)
      last && DateTime.compare(parsed.time, last.time) == :lt -> Repo.rollback(:time_goes_back)
      true -> :ok
    end
  end

  # What an event means for the state of its workload.
  defp advance(workload, %{kind: "started"} = parsed),
    do: Ecto.Changeset.change(workload, state: "preparing", started_at: parsed.time)

  defp advance(workload, %{kind: "workspaceReady"}),
    do: Ecto.Changeset.change(workload, state: "running")

  defp advance(workload, %{kind: "gateStarted"}),
    do: Ecto.Changeset.change(workload, state: "gating")

  defp advance(workload, %{kind: "finished", body: body} = parsed) do
    Ecto.Changeset.change(workload,
      state: Map.fetch!(@outcomes, body["outcome"]),
      reason: body["reason"] || "",
      finished_at: parsed.time
    )
  end

  defp advance(workload, _), do: Ecto.Changeset.change(workload)

  defp parse(event) when is_map(event) do
    with [kind] <- Map.keys(event) -- @envelope,
         %{} = body <- event[kind],
         {sequence, ""} when sequence >= 1 <- Integer.parse(to_string(event["sequence"] || "")),
         {:ok, time, _offset} <- DateTime.from_iso8601(to_string(event["time"] || "")),
         time = to_microsecond_precision(time),
         true <- kind != "finished" or Map.has_key?(@outcomes, body["outcome"]) do
      {:ok,
       %{kind: kind, body: body, sequence: sequence, time: time, workload_id: event["workloadId"]}}
    else
      _ -> {:error, :not_an_event}
    end
  end

  defp parse(_), do: {:error, :not_an_event}

  # The database keeps microseconds, and says so in its type.
  defp to_microsecond_precision(%DateTime{microsecond: {microseconds, _}} = time),
    do: %{time | microsecond: {microseconds, 6}}

  defp belongs_to(%{workload_id: workload_id}, workload_id), do: :ok
  defp belongs_to(_, _), do: {:error, :wrong_workload}
end
