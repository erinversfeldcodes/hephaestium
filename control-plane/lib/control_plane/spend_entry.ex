defmodule ControlPlane.SpendEntry do
  @moduledoc """
  One row of the spend ledger (ADR 0036): what one step of one workload spent, in one unit
  from one source, and the ticket, epic, pull request and branch it concerned. Rows are
  written once and never changed: the database refuses an update or a delete.
  """

  use Ecto.Schema

  @primary_key {:id, :binary_id, autogenerate: true}
  schema "spend_entries" do
    field :workload_id, :binary_id
    field :step, :integer
    field :kind, :string
    field :lens, :string
    field :round, :integer, default: 0
    field :unit, :string
    field :source, :string
    field :amount, :float
    field :input, :float
    field :output, :float
    field :cache_creation, :float
    field :cache_read, :float
    field :repository, :string
    field :ticket_number, :integer
    field :epic_number, :integer
    field :pull_request_number, :integer
    field :branch, :string
    field :occurred_at, :utc_datetime_usec
    field :recorded_at, :utc_datetime_usec
  end
end
