defmodule ControlPlane.Jobs.HandleEvent do
  @moduledoc """
  The work a webhook delivery asks for, as the control plane's own event and nothing the host
  wrote. One job for a delivery id, however often it is inserted. A host that could not be read
  is an error, so the job is tried again; a delivery that is no event is dropped.
  """

  use Oban.Worker,
    queue: :intake,
    max_attempts: 3,
    unique: [keys: [:delivery_id], period: :infinity]

  require Logger

  alias ControlPlane.Webhooks.{Event, Handle}

  @impl Oban.Worker
  def perform(%Oban.Job{args: args}) do
    case Event.from_args(args) do
      {:ok, event} ->
        case Handle.run(event) do
          {:error, message} ->
            Logger.warning("could not handle #{event.kind} of #{event.repository}: #{message}")
            {:error, message}

          _ ->
            :ok
        end

      :error ->
        {:cancel, "the arguments are not an event"}
    end
  end
end
