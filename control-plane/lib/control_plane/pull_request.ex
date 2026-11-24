defmodule ControlPlane.PullRequest do
  @moduledoc """
  A pull request the control plane knows of, with its state (`open`, `merged` or `closed`) and
  the state of its checks (`passing`, `failing`, `pending` or `none`). It is what the code host
  last said, kept so that no page waits on the code host.
  """

  use Ecto.Schema

  @primary_key false
  schema "pull_requests" do
    field :repository, :string, primary_key: true
    field :number, :integer, primary_key: true
    field :url, :string
    field :state, :string
    field :checks, :string
    field :head_sha, :string
    field :changed_at, :utc_datetime_usec
    field :refreshed_at, :utc_datetime_usec
  end
end
