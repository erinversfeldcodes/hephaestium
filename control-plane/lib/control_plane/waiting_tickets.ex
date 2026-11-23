defmodule ControlPlane.WaitingTickets do
  @moduledoc """
  The approved tickets that have no workload yet. Intake writes a ticket here while its
  approval stands and no workload has been made for it, and removes it when a workload starts,
  when the approval stops standing or when the ticket closes. The board reads this table, so
  it never asks the tracker.
  """

  import Ecto.Query

  alias ControlPlane.{Repo, WaitingTicket}

  @doc "Say that a ticket is approved and waits for a workload. A title found earlier is kept when this pass has none."
  @spec put(String.t(), integer(), String.t(), DateTime.t(), String.t() | nil) :: :ok
  def put(repository, number, approved_by, %DateTime{} = approved_at, title \\ nil) do
    now = DateTime.utc_now() |> DateTime.truncate(:second)

    Repo.insert_all(
      WaitingTicket,
      [
        %{
          repository: repository,
          ticket_number: number,
          title: title,
          approved_by: approved_by,
          approved_at: approved_at,
          inserted_at: now,
          updated_at: now
        }
      ],
      on_conflict: [set: [approved_by: approved_by, approved_at: approved_at, updated_at: now]],
      conflict_target: [:repository, :ticket_number]
    )

    if title, do: set_title(repository, number, title)
    :ok
  end

  defp set_title(repository, number, title) do
    Repo.update_all(
      from(w in WaitingTicket, where: w.repository == ^repository and w.ticket_number == ^number),
      set: [title: title]
    )
  end

  @doc "The ticket has a workload, or no approval that stands, or is closed."
  @spec clear(String.t(), integer()) :: :ok
  def clear(repository, number) do
    Repo.delete_all(
      from w in WaitingTicket, where: w.repository == ^repository and w.ticket_number == ^number
    )

    :ok
  end

  @doc "Remove a repository's waiting tickets that are not among the open approved ones."
  @spec keep_only(String.t(), [integer()]) :: :ok
  def keep_only(repository, numbers) do
    Repo.delete_all(
      from w in WaitingTicket,
        where: w.repository == ^repository and w.ticket_number not in ^numbers
    )

    :ok
  end

  @doc "The waiting tickets, the most recently approved first, up to a limit."
  @spec list(non_neg_integer()) :: [%WaitingTicket{}]
  def list(limit),
    do: Repo.all(from w in WaitingTicket, order_by: [desc: w.approved_at], limit: ^limit)
end
