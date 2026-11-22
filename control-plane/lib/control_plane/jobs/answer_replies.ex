defmodule ControlPlane.Jobs.AnswerReplies do
  @moduledoc "Take the next step of the questioning for each ticket whose person has replied, or reopened a ticket Hephaestium closed."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 300]

  @impl Oban.Worker
  def perform(_job) do
    # What no event exists for stays on this pass whatever the webhooks do, so it is not a miss.
    replies = ControlPlane.Gaps.replies()
    ControlPlane.Gaps.unquestioned()
    reopened = ControlPlane.Gaps.reopened()

    ControlPlane.Webhooks.missed(:comment_added, Enum.count(replies, &match?({:started, _}, &1)))

    ControlPlane.Webhooks.missed(
      :ticket_reopened,
      Enum.count(reopened, &match?({:started, _}, &1))
    )

    :ok
  end
end
