defmodule ControlPlane.Workspaces do
  @moduledoc """
  The checkouts that runs leave behind. A run's directory holds what the run said
  (`workload.json`, `events.jsonl`, `executor.log`, `record.json`, all small) and its
  `work/` directory, a full checkout that can be gigabytes. Nothing reads the checkout
  once the run has ended, so it is deleted: at once when the workload completed, and for
  any other end after a window (`workspace_keep_hours`, 72 by default) in which someone
  can look at what the agent left.

  Only a directory named `<ticket>-<workload id>`, under the directory of its
  repository, whose workload has ended, is ever touched; and a symlink is removed,
  never followed.
  """

  require Logger

  alias ControlPlane.{Runner, Workloads}

  @default_keep_hours 72
  @named ~r/\A(\d+)-([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\z/

  @doc """
  Delete every checkout that is due. Returns how many were deleted. A deletion that
  cannot be done is logged and left for the next sweep.
  """
  @spec sweep(DateTime.t()) :: non_neg_integer()
  def sweep(now \\ DateTime.utc_now()) do
    runs = Runner.runs_dir()

    for repository <- directories(runs),
        run <- directories(Path.join(runs, repository)),
        reduce: 0 do
      deleted -> deleted + clean(Path.join([runs, repository, run]), now)
    end
  end

  @doc """
  Delete one run's checkout if it is due: 1 if it was deleted, 0 if not. The directory
  must be a run's own, named as the control plane names it.
  """
  @spec clean(Path.t(), DateTime.t()) :: 0 | 1
  def clean(directory, now \\ DateTime.utc_now()) do
    with {:ok, workload} <- owner(directory),
         true <- due?(workload, now),
         {:ok, work} <- checkout(directory) do
      remove(work)
    else
      _ -> 0
    end
  end

  # The workload a run directory belongs to, when its name and its repository's
  # directory say so.
  defp owner(directory) do
    with [_, number, id] <- Regex.run(@named, Path.basename(directory)),
         %{} = workload <- Workloads.get(id),
         true <- Integer.to_string(workload.ticket_number) == number,
         true <- Path.basename(Path.dirname(directory)) == directory_name(workload.repository),
         {:ok, %File.Stat{type: :directory}} <- File.lstat(directory) do
      {:ok, workload}
    else
      _ -> :none
    end
  end

  defp directory_name(repository), do: String.replace(repository, "/", "-")

  defp due?(%{state: "completed"}, _now), do: true

  defp due?(%{state: state} = workload, now) do
    state in Workloads.terminal_states() and
      DateTime.diff(now, utc(workload.finished_at || workload.updated_at), :second) >=
        keep_seconds()
  end

  # `finished_at` is a DateTime and the schema's own `updated_at` a NaiveDateTime, both UTC.
  defp utc(%DateTime{} = time), do: time
  defp utc(%NaiveDateTime{} = time), do: DateTime.from_naive!(time, "Etc/UTC")

  defp keep_seconds, do: Runner.config(:workspace_keep_hours, @default_keep_hours) * 3600

  defp checkout(directory) do
    work = Path.join(directory, "work")

    case File.lstat(work) do
      {:ok, _} -> {:ok, work}
      {:error, _} -> :none
    end
  end

  defp remove(work) do
    result =
      case File.lstat(work) do
        {:ok, %File.Stat{type: :symlink}} -> File.rm(work)
        {:ok, _} -> work |> File.rm_rf() |> as_result()
        {:error, reason} -> {:error, reason}
      end

    case result do
      :ok ->
        1

      {:error, reason} ->
        Logger.warning("could not delete the checkout #{work}: #{inspect(reason)}")
        0
    end
  end

  defp as_result({:ok, _}), do: :ok
  defp as_result({:error, reason, _file}), do: {:error, reason}

  # The real directories in a directory, by name. A symlink is not one, and a directory
  # that is not there has none.
  defp directories(path) do
    case File.ls(path) do
      {:ok, names} ->
        Enum.filter(names, fn name ->
          match?({:ok, %File.Stat{type: :directory}}, File.lstat(Path.join(path, name)))
        end)

      {:error, _} ->
        []
    end
  end
end
