defmodule ControlPlane.Webhooks.Delivery do
  @moduledoc "A delivery the receiver has taken in, kept so that a redelivery is recognised."

  use Ecto.Schema

  @primary_key {:id, :binary_id, autogenerate: true}
  schema "webhook_deliveries" do
    field :source, :string
    field :delivery_id, :string
    field :received_at, :utc_datetime_usec
  end
end
