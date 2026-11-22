defmodule ControlPlane.TicketLabels do
  @moduledoc """
  The labels the control plane puts on tickets, and making sure a repository has them.

  A person applies `heph:ready` by hand, and the tracker refuses a label it does not know, so
  a repository that has never been served would fail its first approval. The control plane
  therefore creates what is missing the first time it serves a repository after a start, and
  nothing else about the repository's configuration: a label that exists is never changed or
  recreated. The colour and description are the ones the labels already carry in the
  repositories the control plane serves.
  """

  require Logger

  alias ControlPlane.Tracker

  @type label :: %{name: String.t(), colour: String.t(), description: String.t()}

  @quiet "ededed"

  @doc "The labels, with the colour (hex, no leading `#`) and description each carries."
  @spec all() :: [label()]
  def all do
    [
      %{name: "heph:ready", colour: "a1b4a9", description: "Approved for an agent run"},
      %{name: "heph:running", colour: @quiet, description: ""},
      %{name: "heph:done", colour: @quiet, description: ""},
      %{name: "heph:failed", colour: @quiet, description: ""},
      %{name: "heph:blocked", colour: @quiet, description: ""}
    ]
  end

  @doc """
  Ensure the labels exist in the repository, the first time this is called for it since the
  node started. A failure is logged once and does not stop the caller: the repository is not
  tried again until the next start, so one that refuses does not cost a request every pass.
  """
  @spec ensure_once(String.t()) :: :ok
  def ensure_once(repository) do
    key = {__MODULE__, repository}

    if :persistent_term.get(key, false) do
      :ok
    else
      :persistent_term.put(key, true)

      case Tracker.ensure_labels(repository, all()) do
        :ok ->
          :ok

        {:error, message} ->
          Logger.warning("could not ensure the labels of #{repository}: #{message}")
      end

      :ok
    end
  end

  @doc "Check the repository's labels again at its next intake pass."
  @spec forget(String.t()) :: :ok
  def forget(repository) do
    :persistent_term.erase({__MODULE__, repository})
    :ok
  end
end
