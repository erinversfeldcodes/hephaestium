defmodule ControlPlane.Webhooks.Handle do
  @moduledoc """
  What each event does. An event is a hint to look and never the thing looked at: it is
  matched against the database first, and only what belongs to a workload or a ticket the
  control plane has met can cause a read of the code host. The read then gives the current
  state, and the function called is the one the poll calls, so a late, repeated or
  out-of-order delivery changes nothing and a forged claim is no approval.

  The result is what the function called answered, or `{:error, message}` when the host could
  not be read and a later attempt may do better.
  """

  alias ControlPlane.{Evaluations, Gaps, Intake, Settle, Tracker, Workloads}
  alias ControlPlane.Webhooks.Event

  @spec run(Event.t()) :: term()
  def run(%Event{repository: repository} = event) do
    if repository in repositories(), do: handle(event), else: :not_configured
  end

  defp repositories, do: Application.get_env(:control_plane, :intake, [])[:repositories] || []

  # An approval is read from the ticket's history by intake, never from the event.
  defp handle(%Event{kind: :ticket_approved} = event),
    do: Intake.ticket(event.repository, event.number)

  # An edit, or the label coming off, matters to a ticket the control plane has met: one whose
  # workload may still be waiting. A running workload is untouched, as intake leaves it.
  defp handle(%Event{kind: :ticket_edited} = event) do
    if Workloads.for_ticket(event.repository, event.number) == [],
      do: :unknown_ticket,
      else: Intake.ticket(event.repository, event.number)
  end

  defp handle(%Event{kind: :ticket_closed} = event) do
    if Evaluations.enabled?(event.repository) do
      case Tracker.parent(event.repository, event.number) do
        {:ok, nil} -> :no_parent
        {:ok, parent} -> Evaluations.roll_up_parent(event.repository, parent)
        {:error, message} -> {:error, message}
      end
    else
      :not_judged
    end
  end

  defp handle(%Event{kind: :ticket_reopened} = event),
    do: Gaps.reopened(event.repository, event.number)

  # A comment on a change is not a reply to a question.
  defp handle(%Event{kind: :comment_added, on: :change}), do: :not_a_ticket

  defp handle(%Event{kind: :comment_added} = event),
    do: Gaps.replies(event.repository, event.number)

  defp handle(%Event{kind: :base_moved} = event), do: Settle.stale(event.repository)

  # Checks that name no change are matched by the commit they ran on, which the control plane
  # does not keep, so each workload awaiting settlement in the repository is looked at. That is
  # what the poll does, for the few that await.
  defp handle(%Event{kind: :checks_completed, number: nil} = event) do
    for workload <- Workloads.awaiting_settlement(), workload.repository == event.repository do
      Settle.one(workload)
    end
  end

  defp handle(%Event{kind: kind, number: number})
       when kind in [:change_closed, :change_merged, :change_updated, :checks_completed] and
              is_nil(number),
       do: :no_change

  defp handle(%Event{kind: kind} = event)
       when kind in [:checks_completed, :change_closed, :change_merged, :change_updated] do
    for workload <- Workloads.for_change(event.repository, event.number) do
      change(kind, workload)
    end
  end

  # Awaiting settlement: whichever of these happened, the settling looks at the change.
  defp change(_kind, %{settled_at: nil} = workload), do: Settle.one(workload)

  # Settled already: a merge is judged, and a head that moved is kept current. Anything else
  # is nothing to do, because a settled workload is not settled again.
  defp change(:change_merged, workload), do: Evaluations.merged_workload(workload)
  defp change(:change_updated, workload), do: Settle.keep_current(workload)
  defp change(_kind, _workload), do: :settled
end
