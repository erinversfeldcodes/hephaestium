defmodule ControlPlane.RevisionNeeded do
  @moduledoc """
  Whether a revision still has a pull request to revise.

  A revision is queued while its pull request is open, and may start only after a person
  has merged the pull request (and its branch is gone) or closed it. Revising it then is
  impossible, and what the revision says about itself, that it failed, is untrue of the
  work, which merged or was abandoned. So the pull request is asked about before a revision
  starts and again when one has failed. If the code host cannot be asked, the revision is
  taken to be needed: the check must never stop a revision of an open pull request.
  """

  require Logger

  alias ControlPlane.{CodeHost, Workload}

  @doc "`:needed`, or `{:not_needed, reason}` for a revision whose pull request has ended."
  @spec check(%Workload{}) :: :needed | {:not_needed, String.t()}
  def check(%Workload{
        repository: repository,
        workload: %{"revision" => %{"pullRequestNumber" => number}}
      }) do
    case CodeHost.change(repository, number) do
      {:ok, %{merged: true}} ->
        {:not_needed, "the pull request merged before the revision could run"}

      {:ok, %{state: :closed}} ->
        {:not_needed, "the pull request was closed before the revision could run"}

      {:ok, _open} ->
        :needed

      {:error, message} ->
        Logger.warning("could not ask whether #{repository}##{number} is open: #{message}")
        :needed
    end
  end

  def check(%Workload{}), do: :needed
end
