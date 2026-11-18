defmodule ControlPlane.Workloads do
  @moduledoc "Creating and reading workloads."

  import Ecto.Query

  alias ControlPlane.{Repo, Workload}

  @doc """
  Keep a workload, given as the contract's JSON mapping, queued.

  It must be a workload: an id that is a UUID, a repository with an owner and a
  name, and a ticket with a number (a number the mapping writes as a string).
  """
  @spec create(map()) :: {:ok, %Workload{}} | {:error, :not_a_workload | :already_exists}
  def create(%{"id" => id, "repository" => %{"owner" => owner, "name" => name}} = workload)
      when is_binary(owner) and is_binary(name) do
    with {:ok, id} <- Ecto.UUID.cast(id),
         {:ok, number} <- ticket_number(workload) do
      %Workload{
        id: id,
        repository: "#{owner}/#{name}",
        ticket_number: number,
        state: "queued",
        workload: workload
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

  def create(_), do: {:error, :not_a_workload}

  @doc "The workloads of a ticket, oldest first."
  @spec for_ticket(String.t(), integer()) :: [%Workload{}]
  def for_ticket(repository, number) do
    Repo.all(
      from w in Workload,
        where: w.repository == ^repository and w.ticket_number == ^number,
        order_by: w.inserted_at
    )
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

  @terminal ~w(completed failed blocked timed_out cancelled)

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
      set: [state: "failed", reason: reason, finished_at: now, updated_at: now]
    )

    :ok
  end

  @doc "Keep the run record the executor wrote."
  @spec keep_record(Ecto.UUID.t(), map()) :: :ok
  def keep_record(id, record) do
    Repo.update_all(from(w in Workload, where: w.id == ^id),
      set: [record: record, updated_at: DateTime.utc_now()]
    )

    :ok
  end
end
