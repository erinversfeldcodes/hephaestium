defmodule ControlPlane.Health do
  @moduledoc """
  Whether the control plane can do its work, which is to say whether the database answers.
  Everything it keeps, and every job it runs, is in the database, so a control plane that
  cannot reach it is not healthy however well the process itself is.
  """

  # Longer than a healthy answer takes, shorter than a platform's own check gives up.
  @timeout 2_000

  @doc "`:ok` when the database answers a query, otherwise the reason it did not."
  @spec check(module()) :: :ok | {:error, String.t()}
  def check(repo \\ ControlPlane.Repo) do
    case repo.query("select 1", [], timeout: @timeout, queue_target: @timeout) do
      {:ok, _} -> :ok
      {:error, _} -> down()
    end
  rescue
    _ -> down()
  catch
    :exit, _ -> down()
  end

  defp down, do: {:error, "the database did not answer"}
end
