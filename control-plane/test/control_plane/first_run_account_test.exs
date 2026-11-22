defmodule ControlPlane.FirstRunAccountTest do
  @moduledoc """
  What a first run that ended without a pull request found, on the ticket, in the agent's own
  words: as a revision's comment already carries, so that what the agent learned (that it could
  not reproduce the bug, and what to look at next) is not left in the event log.
  """

  use ControlPlane.DataCase

  import ControlPlane.Fixtures

  alias ControlPlane.{Events, StandInGitHub, Tickets, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-first-run-#{System.unique_integer([:positive])}")
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

  # A first run that said what it said, and ended as the outcome says.
  defp ended(id, messages, outcome, reason \\ "") do
    {:ok, _} = Events.append(id, event(id, 1, started()))

    last =
      messages
      |> Enum.with_index(2)
      |> Enum.reduce(1, fn {text, sequence}, _ ->
        {:ok, _} = Events.append(id, event(id, sequence, agent_message(text)))
        sequence
      end)

    {:ok, _} = Events.append(id, event(id, last + 1, finished(outcome, reason)))
    Workloads.keep_record(id, %{"outcome" => outcome})
    Tickets.ended(Workloads.get(id))
  end

  defp comment(gh) do
    {"POST", @issue <> "/comments", ["body=" <> body]} =
      Enum.find(StandInGitHub.writes(gh), fn {method, endpoint, _} ->
        method == "POST" and endpoint == @issue <> "/comments"
      end)

    body
  end

  @could_not_reproduce "I couldn't reproduce the panic, so I made no code change. The next step is the preview app's own log."

  test "a first run that failed says what the agent found, and only its last message", %{
    gh: gh,
    id: id
  } do
    assert :ok =
             ended(
               id,
               ["looking at the handler", @could_not_reproduce],
               "OUTCOME_FAILED",
               "the agent changed nothing, so there is nothing to deliver"
             )

    body = comment(gh)
    assert body =~ "the workload failed"
    assert body =~ "the agent changed nothing"
    assert body =~ "The agent's account"
    assert body =~ @could_not_reproduce
    refute body =~ "looking at the handler"
  end

  test "a first run that timed out or was cancelled says it too", %{gh: gh} do
    for {outcome, text} <- [
          {"OUTCOME_TIMED_OUT", "I ran out of time reading the importer."},
          {"OUTCOME_CANCELLED", "I was stopped part way through the migration."}
        ] do
      {:ok, record} = Workloads.create(workload())
      assert :ok = ended(record.id, [text], outcome, "stopped")
      assert comment_with(gh, text), "#{outcome}: the account is on the ticket"
    end
  end

  defp comment_with(gh, text) do
    StandInGitHub.writes(gh)
    |> Enum.any?(fn
      {"POST", endpoint, ["body=" <> body]} -> endpoint == @issue <> "/comments" and body =~ text
      _ -> false
    end)
  end

  test "a first run whose agent said nothing says there is no account", %{gh: gh, id: id} do
    assert :ok = ended(id, [], "OUTCOME_FAILED", "the agent changed nothing")

    assert comment(gh) =~ "gave no account"
  end

  test "a first run that completed is as it was: its account is on its pull request", %{
    gh: gh,
    id: id
  } do
    assert :ok = ended(id, ["I raised the wait in the spec."], "OUTCOME_COMPLETED")

    body = comment(gh)
    refute body =~ "The agent's account"
    refute body =~ "I raised the wait in the spec."
  end

  test "the account is quoted as data: fenced, so a mention or a command in it does nothing", %{
    gh: gh,
    id: id
  } do
    text = "Ping @someone.\n```\nclose this\n```\n/heph ready\n#99"

    assert :ok = ended(id, [text], "OUTCOME_FAILED", "the agent changed nothing")

    body = comment(gh)
    assert [_, fence] = Regex.run(~r/\n(`{4,})text\n/, body)
    assert body =~ text <> "\n" <> fence
  end
end
