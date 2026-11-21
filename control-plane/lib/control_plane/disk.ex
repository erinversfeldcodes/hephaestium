defmodule ControlPlane.Disk do
  @moduledoc """
  Whether the disk that runs write to has room for another. A run on a disk that is nearly
  full cannot finish, and a disk that fills takes the machine's other work down with it,
  Postgres included.

  The free space is read with `df` (`df` on the path, or the program the runner
  configuration names), asked about the nearest directory of the runs directory that
  exists, since the runs directory is made by the first run.
  """

  require Logger

  alias ControlPlane.{Programs, Runner}

  @default_min_free_mb 10_240

  @doc """
  Whether a run may start: `:ok`, or `{:low, free_mb, min_mb}` when the free space is
  below the limit. A disk that cannot be read is `:ok`, and logged: a fault in measuring
  must not stop all work. A limit of 0 turns the check off.
  """
  @spec room() :: :ok | {:low, non_neg_integer(), pos_integer()}
  def room do
    case Runner.config(:min_free_mb, @default_min_free_mb) do
      0 ->
        :ok

      minimum ->
        case free_mb() do
          {:ok, free} when free < minimum ->
            {:low, free, minimum}

          {:ok, _} ->
            :ok

          {:error, reason} ->
            Logger.warning("could not read the free disk space, so not checking it: #{reason}")
            :ok
        end
    end
  end

  @doc "The free space, in whole megabytes, of the filesystem that holds the runs directory."
  @spec free_mb() :: {:ok, non_neg_integer()} | {:error, String.t()}
  def free_mb do
    command = Runner.config(:df) || System.find_executable("df") || "df"

    if Programs.allowed?(command) do
      read(command, nearest_existing(Runner.runs_dir()))
    else
      {:error, Programs.refusal(command)}
    end
  end

  defp read(command, path) do
    case System.cmd(command, ["-Pk", path], stderr_to_stdout: true) do
      {output, 0} -> parse(output)
      {output, status} -> {:error, "df exited #{status}: #{String.slice(output, 0, 200)}"}
    end
  rescue
    error in ErlangError -> {:error, "could not run df: #{inspect(error.original)}"}
  end

  # The POSIX layout: a header, then `filesystem blocks used available capacity mount`,
  # in kilobytes. A long filesystem name may put the figures on the next line, which
  # `-P` prevents.
  defp parse(output) do
    with [_header, line | _] <- String.split(output, "\n", trim: true),
         [_, _, _, available | _] <- String.split(line),
         {kilobytes, ""} <- Integer.parse(available) do
      {:ok, div(kilobytes, 1024)}
    else
      _ -> {:error, "df's answer is not the POSIX layout: #{String.slice(output, 0, 200)}"}
    end
  end

  defp nearest_existing(path) do
    cond do
      File.dir?(path) -> path
      Path.dirname(path) == path -> path
      true -> nearest_existing(Path.dirname(path))
    end
  end
end
