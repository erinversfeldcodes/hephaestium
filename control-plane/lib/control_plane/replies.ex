defmodule ControlPlane.Replies do
  @moduledoc """
  A person answers the question a run ended blocked with. The answer is a comment on the ticket
  and the ready label applied again, both made with the person's own user token, so
  the label in the ticket's history names the person and intake reads them as the approver and
  checks, as it does for any approval, that they hold write access. Intake then
  starts the new run. The answer is also kept on the workload that asked, before anything is sent,
  and intake puts it in the request of every later workload on the ticket, which is how it reaches
  the agent: nothing here talks to a run.

  A ticket is answered only while its latest run is the one that asked, and only by a person who
  holds write access to the repository: that is checked before anything is sent, because a
  comment needs no such access on a public repository and would otherwise be made by someone
  whose approval would not count. The comment goes first, so a new run never starts without the
  answer.
  """

  alias ControlPlane.{Answers, CodeHost, TicketLabels, Tracker}
  alias ControlPlane.Api.Read

  @max_length 8_000

  @doc "The longest reply, in characters."
  def max_length, do: @max_length

  @type failure ::
          :not_waiting
          | :cannot_write
          | :unsupported
          | :unavailable
          | {:comment | :approval, :unauthorized | :refused | :failed}

  @spec answer(String.t(), pos_integer(), String.t(), String.t(), String.t()) ::
          :ok | {:error, failure()}
  def answer(repository, number, login, token, body) do
    with {:ok, asked} <- waiting(repository, number),
         :ok <- supported(repository),
         :ok <- may_write(repository, login),
         :ok <- Answers.keep(asked.workload, body) do
      send_as(repository, number, token, body)
    end
  end

  defp waiting(repository, number) do
    case Enum.find(Read.questions(), &(&1.repository == repository and &1.ticket == number)) do
      nil -> {:error, :not_waiting}
      asked -> {:ok, asked}
    end
  end

  defp supported(repository) do
    if :act_as_person in Tracker.capabilities(repository),
      do: :ok,
      else: {:error, :unsupported}
  end

  defp may_write(repository, login) do
    case CodeHost.permission(repository, login) do
      {:ok, permission} ->
        if CodeHost.writer?(permission), do: :ok, else: {:error, :cannot_write}

      {:error, _message} ->
        {:error, :unavailable}
    end
  end

  defp send_as(repository, number, token, body) do
    with :ok <- step(:comment, Tracker.comment_as(repository, number, token, body)) do
      step(:approval, Tracker.add_label_as(repository, number, token, TicketLabels.ready()))
    end
  end

  defp step(_name, :ok), do: :ok

  defp step(name, {:error, reason}) when reason in [:unauthorized, :refused],
    do: {:error, {name, reason}}

  defp step(name, {:error, _message}), do: {:error, {name, :failed}}
end
