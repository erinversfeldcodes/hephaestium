defmodule ControlPlane.Event do
  @moduledoc "One thing that happened in a workload, kept whole as the contract wrote it."

  use Ecto.Schema

  schema "events" do
    field :workload_id, :binary_id
    field :sequence, :integer
    field :time, :utc_datetime_usec
    field :kind, :string
    field :event, :map

    timestamps(updated_at: false)
  end
end
