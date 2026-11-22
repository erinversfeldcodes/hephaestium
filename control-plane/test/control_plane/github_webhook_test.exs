defmodule ControlPlane.GitHubWebhookTest do
  @moduledoc """
  What GitHub's deliveries become: the control plane's own events, or nothing. The deliveries
  are synthetic and shaped as GitHub documents them.
  """

  use ExUnit.Case, async: true

  alias ControlPlane.GitHub.Webhook
  alias ControlPlane.Webhooks.Event
  alias ControlPlane.WebhookFixtures, as: W

  @now ~U[2026-11-22 09:00:00Z]

  defp normalise(event, payload) do
    {headers, body} = W.delivery(event, payload, id: "d-1")
    Webhook.normalise(headers, body, @now)
  end

  describe "the signature" do
    test "is the HMAC-SHA256 of the exact body under the secret, and nothing else" do
      {headers, body} = W.delivery("issues", W.labeled())

      assert Webhook.verified?(W.secret(), headers, body)
      refute Webhook.verified?("another-secret", headers, body)
      refute Webhook.verified?(W.secret(), headers, body <> " ")
    end

    test "is refused when it is missing, empty, upper case, unprefixed or the wrong length" do
      {_, body} = W.delivery("issues", W.labeled())
      good = W.sign(body)
      "sha256=" <> digest = good

      for header <- [
            nil,
            "",
            "sha256=",
            String.upcase(good),
            digest,
            "sha1=" <> digest,
            good <> "00",
            "sha256=00"
          ] do
        headers = if header, do: [{"x-hub-signature-256", header}], else: []
        refute Webhook.verified?(W.secret(), headers, body), "accepted #{inspect(header)}"
      end
    end

    test "is never accepted for an empty secret" do
      {_, body} = W.delivery("issues", W.labeled())
      headers = [{"x-hub-signature-256", W.sign(body, "")}]
      refute Webhook.verified?("", headers, body)
    end
  end

  test "the delivery id is the one GitHub gave, and a delivery without one has none" do
    assert Webhook.delivery_id([{"x-github-delivery", "abc"}]) == {:ok, "abc"}
    assert Webhook.delivery_id([]) == :error
    assert Webhook.delivery_id([{"x-github-delivery", ""}]) == :error
  end

  describe "an event" do
    test "of a ticket approved: the ready label applied" do
      assert {:ok, %Event{} = event} = normalise("issues", W.labeled())

      assert %Event{
               kind: :ticket_approved,
               source: "github",
               repository: "erinversfeldcodes/gazebolang",
               number: 12,
               actor: "maint",
               at: @now,
               delivery_id: "d-1"
             } = event
    end

    test "of a ticket edited: its text edited, or the ready label taken off" do
      assert {:ok, %Event{kind: :ticket_edited, cause: :edited, number: 12}} =
               normalise("issues", W.issues("edited"))

      assert {:ok, %Event{kind: :ticket_edited, cause: :unapproved, number: 12}} =
               normalise("issues", W.unlabeled())
    end

    test "of a ticket closed and of a ticket reopened" do
      assert {:ok, %Event{kind: :ticket_closed, number: 12}} =
               normalise("issues", W.issues("closed"))

      assert {:ok, %Event{kind: :ticket_reopened, number: 12}} =
               normalise("issues", W.issues("reopened"))
    end

    test "of a comment, on a ticket or on a change" do
      assert {:ok, %Event{kind: :comment_added, on: :ticket, number: 12}} =
               normalise("issue_comment", W.issue_comment(:ticket))

      assert {:ok, %Event{kind: :comment_added, on: :change, number: 12}} =
               normalise("issue_comment", W.issue_comment(:change))
    end

    test "of a change closed, merged and updated" do
      assert {:ok, %Event{kind: :change_closed, number: 7, head: "abc123"}} =
               normalise("pull_request", W.pull_request("closed"))

      assert {:ok, %Event{kind: :change_merged, number: 7, head: "abc123"}} =
               normalise("pull_request", W.pull_request("closed", true))

      for action <- ["synchronize", "edited"] do
        assert {:ok, %Event{kind: :change_updated, cause: cause, number: 7}} =
                 normalise("pull_request", W.pull_request(action))

        assert cause in [:synchronize, :edited]
      end
    end

    test "of a change updated by a review submitted or dismissed" do
      for action <- ["submitted", "dismissed"] do
        assert {:ok, %Event{kind: :change_updated, cause: :review, number: 7}} =
                 normalise("pull_request_review", W.review(action))
      end
    end

    test "of checks completed, with the change's number when the run names one and the commit always" do
      assert {:ok, %Event{kind: :checks_completed, number: 7, head: "abc123"}} =
               normalise("check_run", W.check_run())

      assert {:ok, %Event{kind: :checks_completed, number: nil, head: "abc123"}} =
               normalise("check_run", W.check_run("completed", []))
    end

    test "of the base moved: a push to the repository's default branch" do
      assert {:ok, %Event{kind: :base_moved, number: nil, head: "def456"}} =
               normalise("push", W.push("refs/heads/main"))
    end

    test "holds nothing a repository wrote" do
      for {event, payload} <- [
            {"issues", W.labeled()},
            {"issue_comment", W.issue_comment()},
            {"pull_request", W.pull_request("closed", true)},
            {"check_run", W.check_run()},
            {"push", W.push()}
          ] do
        assert {:ok, parsed} = normalise(event, payload)
        refute inspect(Event.to_args(parsed)) =~ W.text()
      end
    end

    test "survives being the arguments of a job" do
      {:ok, event} = normalise("pull_request", W.pull_request("closed", true))
      args = event |> Event.to_args() |> Jason.encode!() |> Jason.decode!()

      assert Event.from_args(args) == {:ok, event}
      assert Event.from_args(%{"kind" => "star"}) == :error
      assert Event.from_args(%{}) == :error
    end
  end

  describe "a delivery that is no event" do
    test "of a kind that is not listed, a ping, and installation events" do
      for event <- ["star", "ping", "installation", "installation_repositories", "status", "fork"] do
        assert normalise(event, W.labeled()) == :ignore, event
      end
    end

    test "of an action that is not listed" do
      assert normalise("issues", W.issues("assigned")) == :ignore
      assert normalise("issues", W.issues("milestoned")) == :ignore
      assert normalise("pull_request", W.pull_request("opened")) == :ignore
      assert normalise("pull_request", W.pull_request("labeled")) == :ignore
      assert normalise("pull_request_review", W.review("edited")) == :ignore
      assert normalise("issue_comment", Map.put(W.issue_comment(), "action", "edited")) == :ignore
      assert normalise("check_run", W.check_run("created")) == :ignore
    end

    test "of a label that is not the ready label" do
      assert normalise("issues", W.labeled("bug")) == :ignore
      assert normalise("issues", W.unlabeled("bug")) == :ignore
    end

    test "of a push to a branch that is not the default" do
      assert normalise("push", W.push("refs/heads/heph/12-x")) == :ignore
      assert normalise("push", W.push("refs/tags/v1")) == :ignore
    end
  end

  describe "a delivery that is malformed" do
    test "is an error, not an event and not silence" do
      {headers, _} = W.delivery("issues", W.labeled())

      assert Webhook.normalise(headers, "not json", @now) == :error
      assert Webhook.normalise(headers, "[]", @now) == :error
      assert Webhook.normalise(headers, ~s({"action":"labeled"}), @now) == :error

      assert Webhook.normalise(
               headers,
               Jason.encode!(%{
                 "action" => "closed",
                 "repository" => %{"full_name" => W.repository()}
               }),
               @now
             ) == :error
    end
  end
end
