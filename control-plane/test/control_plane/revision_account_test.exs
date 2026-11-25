defmodule ControlPlane.RevisionAccountTest do
  @moduledoc """
  What a revision found, on the ticket, in the agent's own words (#120, ADR 0024): a
  comment for a revision that ended carries the agent's final message, quoted as data,
  and the failure that follows the last revision lists what each one found.
  """

  use ControlPlane.DataCase

  import Ecto.Query

  import ControlPlane.Fixtures

  alias ControlPlane.{Events, StandInGitHub, Tickets, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @pr "https://github.com/erinversfeldcodes/gazebolang/pull/52"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-account-#{System.unique_integer([:positive])}")
    stand_in = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)

    Application.put_env(:control_plane, :intake,
      gh: stand_in.gh,
      heph: stand_in.heph,
      repositories: []
    )

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      File.rm_rf(dir)
    end)

    {:ok, record} = Workloads.create(workload())
    %{gh: stand_in, id: record.id}
  end

  # The workload is a revision, said what it said, and ended as the outcome says.
  defp revision_ended(id, messages, outcome, reason \\ "", pull_request \\ %{}) do
    current = Workloads.get(id).workload

    ControlPlane.Repo.update_all(
      from(w in ControlPlane.Workload, where: w.id == ^id),
      set: [
        workload:
          Map.put(current, "revision", %{
            "pullRequestNumber" => "52",
            "pullRequestUrl" => @pr,
            "branch" => "heph/12-x",
            "attempt" => 1,
            "failures" => [%{"name" => "ci", "result" => "failure", "excerpt" => "x"}]
          })
      ]
    )

    {:ok, _} = Events.append(id, event(id, 1, started()))

    last =
      messages
      |> Enum.with_index(2)
      |> Enum.reduce(1, fn {text, sequence}, _ ->
        {:ok, _} = Events.append(id, event(id, sequence, agent_message(text)))
        sequence
      end)

    {:ok, _} = Events.append(id, event(id, last + 1, finished(outcome, reason)))

    Workloads.keep_record(id, %{
      "outcome" => outcome,
      "pullRequest" => Map.merge(%{"url" => @pr, "number" => "52"}, pull_request)
    })

    Tickets.ended(Workloads.get(id))
  end

  defp comment(gh) do
    {"POST", @issue <> "/comments", ["body=" <> body]} =
      Enum.find(StandInGitHub.writes(gh), fn {method, endpoint, _} ->
        method == "POST" and endpoint == @issue <> "/comments"
      end)

    body
  end

  test "a revision that changed nothing says what the agent found, in the agent's words", %{
    gh: gh,
    id: id
  } do
    assert :ok =
             revision_ended(
               id,
               ["looking", "The failing spec is invite.spec.ts. I made no change."],
               "OUTCOME_FAILED",
               "the agent changed nothing, so there is nothing to deliver"
             )

    assert {"POST", _, ["labels[]=heph:failed"]} = hd(StandInGitHub.writes(gh))
    body = comment(gh)
    assert body =~ "the revision failed"
    assert body =~ "the agent changed nothing"
    assert body =~ "The agent's account"
    assert body =~ "The failing spec is invite.spec.ts. I made no change."
    refute body =~ "looking", "only the last message is the account"
    assert body =~ @pr
  end

  test "a revision that pushed says what the agent says it did", %{gh: gh, id: id} do
    assert :ok =
             revision_ended(id, ["I raised the wait in the spec to 30s."], "OUTCOME_COMPLETED")

    body = comment(gh)
    assert body =~ "revision"
    assert body =~ "The agent's account"
    assert body =~ "I raised the wait in the spec to 30s."
  end

  test "a revision that pushed nothing says the description was updated and nothing was pushed",
       %{gh: gh, id: id} do
    assert :ok =
             revision_ended(id, ["I corrected the description."], "OUTCOME_COMPLETED", "", %{
               "nothingPushed" => true
             })

    body = comment(gh)
    assert body =~ "updated the pull request's description"
    assert body =~ "nothing was pushed"
    refute body =~ "completed and pushed"
  end

  test "a revision whose agent said nothing says there is no account", %{gh: gh, id: id} do
    assert :ok = revision_ended(id, [], "OUTCOME_FAILED", "the agent changed nothing")

    body = comment(gh)
    assert body =~ "gave no account"
    refute body =~ "The agent's account"
  end

  test "the account is quoted as data: fenced, so a mention or a command in it does nothing",
       %{gh: gh, id: id} do
    text = "Ping @someone.\n```\nclose this\n```\n/heph ready\n#99"

    assert :ok = revision_ended(id, [text], "OUTCOME_FAILED", "the agent changed nothing")

    body = comment(gh)
    # The fence is longer than any run of backticks in the text, so the text cannot end it.
    assert [_, fence] = Regex.run(~r/\n(`{4,})text\n/, body)
    assert body =~ text <> "\n" <> fence
    # And what follows the account is ours.
    assert String.ends_with?(body, "Pull request: " <> @pr) or body =~ "Pull request: " <> @pr
  end

  test "a long account is cut, and says it was", %{gh: gh, id: id} do
    long = String.duplicate("a long line of reasoning. ", 400)

    assert :ok = revision_ended(id, [long], "OUTCOME_FAILED", "the agent changed nothing")

    body = comment(gh)
    assert byte_size(body) < 4_000
    assert body =~ "cut"
    assert body =~ "a long line of reasoning."
  end

  test "terminal escapes and other control characters are dropped", %{gh: gh, id: id} do
    assert :ok =
             revision_ended(
               id,
               ["red \e[31mtext\e[0m and a bell \a and a form feed \f end"],
               "OUTCOME_FAILED",
               "the agent changed nothing"
             )

    body = comment(gh)
    refute body =~ "\e"
    refute body =~ "\a"
    refute body =~ "\f"
    # The whole escape sequence goes, not only its first byte.
    refute body =~ "[31m"
    refute body =~ "[0m"
    assert body =~ "red text and a bell"
  end

  test "a message that is only whitespace is no account", %{gh: gh, id: id} do
    assert :ok = revision_ended(id, ["  \n\t \n"], "OUTCOME_FAILED", "the agent changed nothing")

    body = comment(gh)
    assert body =~ "gave no account"
    refute body =~ "The agent's account"
  end

  # A first run that failed does show its account (see first_run_account_test.exs); one that
  # completed has its account on its pull request, so the ticket's comment has none.
  test "a first run that completed gets no account on the ticket", %{gh: gh, id: id} do
    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, agent_message("I did the work.")))
    {:ok, _} = Events.append(id, event(id, 3, finished("OUTCOME_COMPLETED", "")))
    Workloads.keep_record(id, %{"outcome" => "OUTCOME_COMPLETED"})

    assert :ok = Tickets.ended(Workloads.get(id))

    refute comment(gh) =~ "I did the work."
    refute comment(gh) =~ "account"
  end

  test "a revision that is blocked keeps its question and gets no second account", %{
    gh: gh,
    id: id
  } do
    current = Workloads.get(id).workload

    ControlPlane.Repo.update_all(
      from(w in ControlPlane.Workload, where: w.id == ^id),
      set: [workload: Map.put(current, "revision", %{"attempt" => 1})]
    )

    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, agent_message("Which environment is it?")))
    {:ok, _} = Events.append(id, event(id, 3, finished("OUTCOME_BLOCKED", "needs an answer")))
    Workloads.keep_record(id, %{"outcome" => "OUTCOME_BLOCKED", "question" => "Which one?"})

    assert :ok = Tickets.ended(Workloads.get(id))

    body = comment(gh)
    assert body =~ "A question for you:\n\n```text\nWhich one?\n```"
    refute body =~ "The agent's account"
  end
end
