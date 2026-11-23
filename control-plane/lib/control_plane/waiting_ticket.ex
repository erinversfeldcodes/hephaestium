defmodule ControlPlane.WaitingTicket do
  @moduledoc "An approved ticket that has no workload yet, and when its approval was given."

  use Ecto.Schema

  @primary_key false
  schema "waiting_tickets" do
    field :repository, :string, primary_key: true
    field :ticket_number, :integer, primary_key: true
    field :title, :string
    field :approved_by, :string
    field :approved_at, :utc_datetime

    timestamps(type: :utc_datetime)
  end
end
