defmodule ControlPlane.SettleAnnouncement do
  @moduledoc """
  The claim to announce a pull request's state once for a head commit. A ticket keeps a workload
  for every attempt and each can look at the same pull request, so what stops the same comment
  being written again is a row that only one of them can insert.
  """

  use Ecto.Schema

  alias ControlPlane.Repo

  @primary_key false
  schema "settle_announcements" do
    field :repository, :string, primary_key: true
    field :number, :integer, primary_key: true
    field :head_sha, :string, primary_key: true
    field :state, :string, primary_key: true
    field :announced_at, :utc_datetime_usec
  end

  @doc "Claim the announcement. `true` for the one caller that got it, `false` for any other."
  @spec claim(String.t(), integer(), String.t(), String.t()) :: boolean()
  def claim(repository, number, head_sha, state) do
    {count, _} =
      Repo.insert_all(
        __MODULE__,
        [
          %{
            repository: repository,
            number: number,
            head_sha: head_sha,
            state: state,
            announced_at: DateTime.utc_now()
          }
        ],
        on_conflict: :nothing
      )

    count == 1
  end

  @doc "Give the claim up, because the announcement could not be written and is to be tried again."
  @spec release(String.t(), integer(), String.t(), String.t()) :: :ok
  def release(repository, number, head_sha, state) do
    import Ecto.Query

    Repo.delete_all(
      from(a in __MODULE__,
        where:
          a.repository == ^repository and a.number == ^number and a.head_sha == ^head_sha and
            a.state == ^state
      )
    )

    :ok
  end
end
