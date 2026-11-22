defmodule ControlPlane.AgentTextPointerTest do
  @moduledoc """
  Both repositories are public, so what an agent wrote is read behind the login. When the
  dashboard's address is configured, the comment on the ticket (and on the pull request) says
  the outcome, what was spent and where to read the run, and carries none of the agent's words.
  When it is not configured the comment is what it always was: nothing is taken off the ticket
  for a link that does not exist.
  """

  use ControlPlane.DataCase

  import Ecto.Query
  import ControlPlane.Fixtures

  alias ControlPlane.{Events, StandInGitHub, Tickets, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @pr "https://github.com/#{@repo}/pull/52"
  @dashboard "https://heph.example.test"

  @account "I read lib/secret_plan.ex and the payroll gap is the cause."
  @question "Should I touch /srv/private/keys.pem?"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-pointer-#{System.unique_integer([:positive])}")
    stand_in = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)

    Application.put_env(:control_plane, :intake,
      gh: stand_in.gh,
      heph: stand_in.heph,
      repositories: []
    )

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      Application.delete_env(:control_plane, :dashboard)
      File.rm_rf(dir)
    end)

    {:ok, record} = Workloads.create(workload())
    %{gh: stand_in, id: record.id}
  end

  defp configure, do: Application.put_env(:control_plane, :dashboard, url: @dashboard)

  defp as_revision(id) do
    current = Workloads.get(id).workload

    ControlPlane.Repo.update_all(from(w in ControlPlane.Workload, where: w.id == ^id),
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
  end

  defp end_run(id, outcome, record \\ %{}) do
    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, agent_message(@account)))
    {:ok, _} = Events.append(id, event(id, 3, finished(outcome, "stopped")))

    Workloads.keep_record(
      id,
      Map.merge(
        %{
          "outcome" => outcome,
          "costs" => [%{"unit" => "COST_UNIT_USD", "amount" => 0.5}]
        },
        record
      )
    )

    Tickets.ended(Workloads.get(id))
  end

  defp comments(gh, endpoint) do
    for {"POST", ^endpoint, ["body=" <> body]} <- StandInGitHub.writes(gh), do: body
  end

  defp ticket_comment(gh), do: gh |> comments(@issue <> "/comments") |> hd()

  defp link(id), do: "#{@dashboard}/tickets/#{@repo}/12#run-#{id}"

  describe "with the dashboard's address configured" do
    setup do
      configure()
    end

    test "a failed first run says outcome, cost and link, and none of the agent's words",
         %{gh: gh, id: id} do
      assert :ok = end_run(id, "OUTCOME_FAILED")

      body = ticket_comment(gh)
      assert body =~ "the workload failed"
      assert body =~ "$0.50"
      assert body =~ link(id)
      refute body =~ "secret_plan"
      refute body =~ "payroll"
      refute body =~ "The agent's account of what it found"
    end

    test "a blocked first run points to its question and carries none of it", %{gh: gh, id: id} do
      assert :ok = end_run(id, "OUTCOME_BLOCKED", %{"question" => @question})

      body = ticket_comment(gh)
      assert body =~ "the workload is blocked"
      assert body =~ link(id)
      assert body =~ "ready"
      refute body =~ "keys.pem"
      refute body =~ "A question for you:\n\n```"
      refute body =~ "secret_plan"
    end

    test "a revision says outcome and link on the ticket and on its pull request", %{
      gh: gh,
      id: id
    } do
      as_revision(id)

      assert :ok =
               end_run(id, "OUTCOME_FAILED", %{"pullRequest" => %{"url" => @pr, "number" => "52"}})

      ticket = ticket_comment(gh)
      assert ticket =~ "the revision failed"
      assert ticket =~ link(id)
      refute ticket =~ "secret_plan"

      assert [pull_request] = comments(gh, "repos/#{@repo}/issues/52/comments")
      assert pull_request =~ "revision 1"
      assert pull_request =~ link(id)
      refute pull_request =~ "secret_plan"
    end
  end

  describe "without the dashboard's address" do
    test "the account and the question stay on the ticket, as they were", %{gh: gh, id: id} do
      assert :ok = end_run(id, "OUTCOME_FAILED")
      body = ticket_comment(gh)
      assert body =~ "The agent's account of what it found"
      assert body =~ "secret_plan"
      refute body =~ "#run-"
    end

    test "a blocked run's question stays on the ticket", %{gh: gh, id: id} do
      assert :ok = end_run(id, "OUTCOME_BLOCKED", %{"question" => @question})
      body = ticket_comment(gh)
      assert body =~ "A question for you:"
      assert body =~ "keys.pem"
    end

    test "an address that is not set to anything counts as not configured", %{gh: gh, id: id} do
      Application.put_env(:control_plane, :dashboard, url: "")
      assert :ok = end_run(id, "OUTCOME_FAILED")
      assert ticket_comment(gh) =~ "secret_plan"
    end
  end
end
