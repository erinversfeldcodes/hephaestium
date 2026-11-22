defmodule ControlPlane.WorkloadLease do
  @moduledoc "Which executor holds a workload, until when, and how often it has been put back."

  use Ecto.Schema

  @primary_key {:workload_id, :binary_id, autogenerate: false}
  schema "workload_leases" do
    field :token_id, :binary_id
    field :active, :boolean, default: false
    field :expires_at, :utc_datetime_usec
    field :requeues, :integer, default: 0

    timestamps(type: :utc_datetime_usec)
  end
end
