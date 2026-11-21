defmodule ControlPlane.Jobs.AnswerReplies do
  @moduledoc "Take the next step of the questioning for each ticket whose person has replied, or reopened a ticket Hephaestium closed."

  use Oban.Worker, queue: :intake, max_attempts: 1, unique: [period: 300]

  @impl Oban.Worker
  def perform(_job) do
    ControlPlane.Gaps.replies()
    ControlPlane.Gaps.reopened()
    :ok
  end
end
