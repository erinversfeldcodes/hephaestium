defmodule ControlPlane.Workloads do
  @moduledoc "Creating and reading workloads."

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
end
