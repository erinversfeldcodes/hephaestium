defmodule ControlPlane.DeviceToken do
  @moduledoc "A token that lets one person's executor claim that person's workloads."

  use Ecto.Schema

  @primary_key {:id, :binary_id, autogenerate: true}
  schema "device_tokens" do
    field :login, :string
    field :name, :string
    field :token_hash, :string, redact: true
    field :last_seen_at, :utc_datetime_usec
    field :revoked_at, :utc_datetime_usec

    timestamps(updated_at: false, type: :utc_datetime_usec)
  end
end
