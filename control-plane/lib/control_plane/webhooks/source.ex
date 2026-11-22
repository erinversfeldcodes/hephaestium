defmodule ControlPlane.Webhooks.Source do
  @moduledoc """
  A code host or a tracker that sends webhooks, as the receiver sees it: how it signs a
  delivery, how it names one, and how what it sent becomes the control plane's own event.
  Each system's receiver is written for it and emits the same events, so the handlers do not
  know who sent them.
  """

  alias ControlPlane.Webhooks.Event

  @type headers :: [{String.t(), String.t()}]

  @doc "Whether the delivery is signed with the secret, over exactly the bytes received."
  @callback verified?(secret :: binary(), headers, body :: binary()) :: boolean()

  @doc "The id the sender gave the delivery, which a redelivery repeats."
  @callback delivery_id(headers) :: {:ok, String.t()} | :error

  @doc "The event the delivery is, nothing if it is of no interest, or an error if it is malformed."
  @callback normalise(headers, body :: binary(), now :: DateTime.t()) ::
              {:ok, Event.t()} | :ignore | :error
end
