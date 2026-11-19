defmodule ControlPlane.PullRequestCommentsTest do
  @moduledoc """
  What a review found and what a revision's agent answered, on the pull request where a
  person reviewing it looks (#127). Comments only: nothing is submitted as a GitHub
  approval or a request for changes, because the repository's approval rule is for a person.
  """

  use ControlPlane.DataCase

  import Ecto.Query

  import ControlPlane.Fixtures

  alias ControlPlane.{Events, StandInGitHub, Tickets, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @pr_comments "repos/#{@repo}/issues/52/comments"
  @ticket_comments "repos/#{@repo}/issues/12/comments"
  @pr_url "https://github.com/#{@repo}/pull/52"
  @sha "f6f2dc5f9df5fb32ff2a5c36ebee4e3bc9e159d5"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-prc-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)
    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [])

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      File.rm_rf(dir)
    end)

    {:ok, parent} = Workloads.create(workload())
    %{gh: gh, parent: parent}
  end

  # A review workload of the parent, under this lens, that ended as the outcome says and
  # kept this verdict (or none).
  defp review(parent, lens, outcome, verdict, reason \\ "") do
    review =
      parent.workload
      |> Map.put("id", Ecto.UUID.generate())
      |> Map.put("review", %{
        "pullRequestNumber" => "52",
        "pullRequestUrl" => @pr_url,
        "lens" => lens,
        "lensDocument" => "the #{lens} lens"
      })

    {:ok, record} = Workloads.create(review, parent: parent.id)
    {:ok, _} = Events.append(record.id, event(record.id, 1, started()))
    {:ok, _} = Events.append(record.id, event(record.id, 2, finished(outcome, reason)))

    kept = %{"outcome" => outcome}

    Workloads.keep_record(
      record.id,
      if(verdict, do: Map.put(kept, "review", verdict), else: kept)
    )

    Workloads.get(record.id)
  end

  defp verdict(kind, findings \\ [], notes \\ ["a note"]),
    do: %{
      "schema_version" => 0.0,
      "lens" => "rust",
      "commit" => @sha,
      "verdict" => kind,
      "findings" => findings,
      "notes" => notes
    }

  defp finding(overrides \\ %{}),
    do:
      Map.merge(
        %{
          "severity" => "P1",
          "blocking" => true,
          "title" => "Causes are asserted, not shown",
          "evidence" => "FLAKES.md says the cause is the best reading of the logs",
          "revision" => "Quote the failing run's log for each spec.",
          "location" => %{"path" => "e2e/FLAKES.md", "line" => 28.0}
        },
        overrides
      )

  defp writes(gh, endpoint),
    do: for({"POST", ^endpoint, [field]} <- StandInGitHub.writes(gh), do: field)

  defp pr_comments(gh),
    do: Enum.map(writes(gh, @pr_comments), &String.replace_prefix(&1, "body=", ""))

  test "a review that found something is one comment on the pull request, saying what", %{
    gh: gh,
    parent: parent
  } do
    workload = review(parent, "rust", "OUTCOME_COMPLETED", verdict("needs_revision", [finding()]))

    assert :ok = Tickets.ended(workload)

    assert [body] = pr_comments(gh)
    assert body =~ "review"
    assert body =~ "rust"
    assert body =~ String.slice(@sha, 0, 7)
    assert body =~ "needs_revision"
    assert body =~ "P1"
    assert body =~ "blocking"
    assert body =~ "Causes are asserted, not shown"
    assert body =~ "e2e/FLAKES.md:28"
    refute body =~ "28.0"
    assert body =~ "FLAKES.md says the cause is the best reading of the logs"
    assert body =~ "Quote the failing run's log for each spec."
  end

  test "an approval is a comment too, with what the reviewer noted", %{gh: gh, parent: parent} do
    workload =
      review(
        parent,
        "rust",
        "OUTCOME_COMPLETED",
        verdict("approved", [], ["No Rust file changed, so no axis applies."])
      )

    assert :ok = Tickets.ended(workload)

    assert [body] = pr_comments(gh)
    assert body =~ "approved"
    assert body =~ "No Rust file changed, so no axis applies."
  end

  test "a finding that does not block is shown as not blocking", %{gh: gh, parent: parent} do
    workload =
      review(
        parent,
        "rust",
        "OUTCOME_COMPLETED",
        verdict("approved", [finding(%{"severity" => "P3", "blocking" => false})])
      )

    assert :ok = Tickets.ended(workload)

    assert [body] = pr_comments(gh)
    assert body =~ "P3"
    assert body =~ "not blocking"
  end

  test "what the agent wrote is data: fenced, so a mention, a link or a fence in it does nothing",
       %{gh: gh, parent: parent} do
    nasty =
      finding(%{
        "title" => "Ping @someone and close #99",
        "evidence" => "```\nend of the block\n```\n/heph ready",
        "revision" => "<script>alert(1)</script>"
      })

    workload =
      review(
        parent,
        "rust",
        "OUTCOME_COMPLETED",
        verdict("needs_revision", [nasty], ["@everyone"])
      )

    assert :ok = Tickets.ended(workload)

    assert [body] = pr_comments(gh)
    # Every run of backticks the agent wrote is shorter than the fence that holds it.
    assert Regex.match?(~r/\n`{4,}text\n/, body), "the finding is not in a longer fence:\n#{body}"
    assert length(Regex.scan(~r/\n`{3,}text\n/, body)) >= 2
    refute Regex.match?(~r/(^|\n)@someone/, body), "a mention outside a fence:\n#{body}"
    refute Regex.match?(~r/(^|\n)@everyone/, body), "a mention outside a fence:\n#{body}"
    assert body =~ "```\nend of the block\n```\n/heph ready"
  end

  test "a severity or lens that is not what the contract says is not repeated as it is",
       %{gh: gh, parent: parent} do
    workload =
      review(
        parent,
        "rust",
        "OUTCOME_COMPLETED",
        verdict("needs_revision", [finding(%{"severity" => "P9 @someone ```"})])
      )

    assert :ok = Tickets.ended(workload)

    assert [body] = pr_comments(gh)
    refute body =~ "P9"
    refute body =~ "@someone ```"
  end

  test "a review that could not run says so, and is not an approval", %{gh: gh, parent: parent} do
    workload = review(parent, "rust", "OUTCOME_FAILED", nil, "the agent changed a file")

    assert :ok = Tickets.ended(workload)

    assert [body] = pr_comments(gh)
    assert body =~ "rust"
    assert body =~ "could not review"
    assert body =~ "the agent changed a file"
    assert body =~ "That is no approval."
    refute body =~ "approved"
  end

  test "a review that completed and left no verdict says that", %{gh: gh, parent: parent} do
    workload = review(parent, "rust", "OUTCOME_COMPLETED", nil)

    assert :ok = Tickets.ended(workload)

    assert [body] = pr_comments(gh)
    assert body =~ "left no verdict"
    assert body =~ "That is no approval."
  end

  test "a lens name that is not a pack name is not repeated as it is", %{gh: gh, parent: parent} do
    review = review(parent, "rust` @someone ```", "OUTCOME_COMPLETED", verdict("approved"))

    assert :ok = Tickets.ended(review)

    assert [body] = pr_comments(gh)
    refute body =~ "@someone"
    assert body =~ "`a lens`"
  end

  test "a review writes its comment once, however often it is ended", %{gh: gh, parent: parent} do
    workload = review(parent, "rust", "OUTCOME_COMPLETED", verdict("approved"))

    assert :ok = Tickets.ended(workload)
    assert :ok = Tickets.ended(Workloads.get(workload.id))

    assert [_] = pr_comments(gh)
  end

  test "it is a comment: nothing is submitted as a review, an approval or a request for changes",
       %{gh: gh, parent: parent} do
    workload = review(parent, "rust", "OUTCOME_COMPLETED", verdict("needs_revision", [finding()]))

    assert :ok = Tickets.ended(workload)

    for {_method, endpoint, _fields} <- StandInGitHub.writes(gh) do
      refute endpoint =~ "/reviews", endpoint
      refute endpoint =~ "/pulls/", endpoint
    end
  end

  test "a review says nothing on the ticket by itself, as before", %{gh: gh, parent: parent} do
    workload = review(parent, "rust", "OUTCOME_COMPLETED", verdict("approved"))

    assert :ok = Tickets.ended(workload)

    assert writes(gh, @ticket_comments) == []
  end

  test "a write that fails is logged, changes nothing about how the review ended, and is tried again",
       %{gh: gh, parent: parent} do
    workload = review(parent, "rust", "OUTCOME_COMPLETED", verdict("approved"))
    StandInGitHub.fail_write(gh, "POST", @pr_comments, "gh: HTTP 403 Forbidden")

    assert {:error, message} = Tickets.ended(workload)

    assert message =~ "403"
    assert Workloads.get(workload.id).state == "completed"
    # The claim was given back, so the next attempt is not turned away.
    refute Workloads.get(workload.id).reported_at
    assert length(pr_comments(gh)) == 1, "one attempt"

    assert {:error, _} = Tickets.ended(Workloads.get(workload.id))
    assert length(pr_comments(gh)) == 2, "the second attempt was made"
  end

  test "a revision's account is on the pull request as well as the ticket", %{
    gh: gh,
    parent: parent
  } do
    revision =
      parent.workload
      |> Map.put("id", Ecto.UUID.generate())
      |> Map.put("revision", %{
        "pullRequestNumber" => "52",
        "pullRequestUrl" => @pr_url,
        "branch" => "heph/12-x",
        "attempt" => 1,
        "failures" => [%{"name" => "ci", "result" => "failure", "excerpt" => "x"}]
      })

    {:ok, record} = Workloads.create(revision, parent: parent.id)
    {:ok, _} = Events.append(record.id, event(record.id, 1, started()))

    {:ok, _} =
      Events.append(record.id, event(record.id, 2, agent_message("I could not show the cause.")))

    {:ok, _} = Events.append(record.id, event(record.id, 3, finished("OUTCOME_COMPLETED", "")))

    Workloads.keep_record(record.id, %{
      "outcome" => "OUTCOME_COMPLETED",
      "pullRequest" => %{"url" => @pr_url, "number" => "52"}
    })

    assert :ok = Tickets.ended(Workloads.get(record.id))

    assert [body] = pr_comments(gh)
    assert body =~ "revision 1"
    assert body =~ "I could not show the cause."
    assert [_] = writes(gh, @ticket_comments)
  end

  test "a revision that stopped to ask, or was cancelled, writes no account on the pull request",
       %{gh: gh, parent: parent} do
    for outcome <- ["OUTCOME_BLOCKED", "OUTCOME_CANCELLED"] do
      revision =
        parent.workload
        |> Map.put("id", Ecto.UUID.generate())
        |> Map.put("revision", %{
          "pullRequestNumber" => "52",
          "pullRequestUrl" => @pr_url,
          "branch" => "heph/12-x",
          "attempt" => 1,
          "failures" => []
        })

      {:ok, record} = Workloads.create(revision, parent: parent.id)
      {:ok, _} = Events.append(record.id, event(record.id, 1, started()))
      {:ok, _} = Events.append(record.id, event(record.id, 2, agent_message("an account")))
      {:ok, _} = Events.append(record.id, event(record.id, 3, finished(outcome, "x")))
      Workloads.keep_record(record.id, %{"outcome" => outcome, "question" => "Which one?"})

      Tickets.ended(Workloads.get(record.id))
    end

    assert pr_comments(gh) == []
  end

  test "a pull request number that is not a number writes nothing, and is an error", %{
    gh: gh,
    parent: parent
  } do
    for bad <- ["52/../../x", "", "0", "-1", "52 ", "5e2", nil, 52] do
      review =
        parent.workload
        |> Map.put("id", Ecto.UUID.generate())
        |> Map.put("review", %{
          "pullRequestNumber" => bad,
          "pullRequestUrl" => @pr_url,
          "lens" => "rust",
          "lensDocument" => "x"
        })

      {:ok, record} = Workloads.create(review, parent: parent.id)
      {:ok, _} = Events.append(record.id, event(record.id, 1, started()))
      {:ok, _} = Events.append(record.id, event(record.id, 2, finished("OUTCOME_COMPLETED", "")))

      Workloads.keep_record(record.id, %{
        "outcome" => "OUTCOME_COMPLETED",
        "review" => verdict("approved")
      })

      assert {:error, message} = Tickets.ended(Workloads.get(record.id)), inspect(bad)
      assert message =~ "is not one", inspect(bad)
    end

    assert StandInGitHub.writes(gh) == []
  end

  test "a lens name, a commit and a verdict word that are not what the contract says are not repeated",
       %{gh: gh, parent: parent} do
    odd =
      Map.merge(verdict("approved"), %{
        "commit" => "```@someone",
        "verdict" => "approved @someone"
      })

    review = review(parent, "rust", "OUTCOME_COMPLETED", odd)

    assert :ok = Tickets.ended(review)

    assert [body] = pr_comments(gh)
    refute body =~ "@someone"
    assert body =~ "an unrecognised verdict"
    assert body =~ "a commit"
  end

  test "a workload that is neither a review nor a revision writes nothing on the pull request",
       %{gh: gh, parent: parent} do
    {:ok, _} = Events.append(parent.id, event(parent.id, 1, started()))
    {:ok, _} = Events.append(parent.id, event(parent.id, 2, finished("OUTCOME_FAILED", "x")))
    Workloads.keep_record(parent.id, %{"outcome" => "OUTCOME_FAILED"})

    assert :ok = Tickets.ended(Workloads.get(parent.id))

    assert pr_comments(gh) == []
  end

  test "the claim that a comment was written is kept on the workload", %{parent: parent} do
    workload = review(parent, "rust", "OUTCOME_COMPLETED", verdict("approved"))

    assert :ok = Tickets.ended(workload)

    assert %{reported_at: %DateTime{}} = Workloads.get(workload.id)

    assert 1 ==
             ControlPlane.Repo.aggregate(
               from(w in ControlPlane.Workload, where: not is_nil(w.reported_at)),
               :count
             )
  end
end
