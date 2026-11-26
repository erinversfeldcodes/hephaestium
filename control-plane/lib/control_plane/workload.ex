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
    field :provenance, :map
    field :reason, :string
    field :started_at, :utc_datetime_usec
    field :finished_at, :utc_datetime_usec
    field :settled_at, :utc_datetime_usec
    field :reported_at, :utc_datetime_usec
    field :report_url, :string
    field :resumed_for, :utc_datetime_usec
    field :budget_base, :integer, default: 0
    field :reruns, :map, default: %{}
    field :unread_logs, :map, default: %{}
    field :parent_id, :binary_id
    field :answer, :string

    timestamps()
  end
end
