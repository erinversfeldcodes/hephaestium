defmodule ControlPlane.Runner do
  @moduledoc """
  Running workloads: putting one on the queue, stopping one that runs, and how the
  executor that runs them is configured.
  """

  alias ControlPlane.Jobs.RunWorkload

  @doc "Put a workload on the queue. However often it is asked, it is queued once."
  @spec enqueue(Ecto.UUID.t()) :: {:ok, Oban.Job.t()} | {:error, term()}
  def enqueue(workload_id),
    do: %{"workload_id" => workload_id} |> RunWorkload.new() |> Oban.insert()

  @doc """
  Tell a running workload's executor to stop. The executor stops what it started
  and says the workload ended cancelled, which the record keeps as it keeps
  anything the executor says.
  """
  @spec cancel(Ecto.UUID.t()) :: :ok | {:error, :not_running}
  def cancel(workload_id) do
    case Registry.lookup(ControlPlane.RunnerRegistry, workload_id) do
      [{pid, _}] ->
        send(pid, :cancel)
        :ok

      [] ->
        {:error, :not_running}
    end
  end

  @doc false
  def config(key, default \\ nil),
    do: :control_plane |> Application.get_env(:runner, []) |> Keyword.get(key, default)
end
