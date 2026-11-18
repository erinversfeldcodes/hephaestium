defmodule ControlPlane.Workload do
  @moduledoc "A workload, and where it has got to."

  use Ecto.Schema

  @primary_key {:id, :binary_id, autogenerate: false}
  schema "workloads" do
    field :repository, :string
    field :ticket_number, :integer
    field :state, :string, default: "queued"
    field :workload, :map
    field :record, :map
    field :reason, :string
    field :started_at, :utc_datetime_usec
    field :finished_at, :utc_datetime_usec
    field :settled_at, :utc_datetime_usec
    field :reruns, :map, default: %{}
    field :parent_id, :binary_id

    timestamps()
  end
end
