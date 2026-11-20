defmodule ControlPlane.TicketsTest do
  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import Ecto.Query

  import ControlPlane.Fixtures

  alias ControlPlane.{Events, StandInGitHub, Tickets, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @pr "https://github.com/erinversfeldcodes/gazebolang/pull/52"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-tickets-#{System.unique_integer([:positive])}")
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

  # A workload that has ended as the outcome says, with the record the executor
  # would have kept.
  defp ended(id, outcome, reason, extra \\ %{}) do
    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, finished(outcome, reason)))

    Workloads.keep_record(
      id,
      Map.merge(
        %{
          "outcome" => outcome,
          "costs" => [
            %{
              "unit" => "COST_UNIT_USD",
              "amount" => 0.126,
              "source" => "SOURCE_REPORTED_BY_RUNTIME"
            },
            %{
              "unit" => "COST_UNIT_TOKENS",
              "amount" => 144_096,
              "source" => "SOURCE_REPORTED_BY_RUNTIME"
            }
          ]
        },
        extra
      )
    )

    Tickets.ended(Workloads.get(id))
  end

  defp comment(gh) do
    {"POST", @issue <> "/comments", ["body=" <> body]} =
      Enum.find(StandInGitHub.writes(gh), fn {method, endpoint, _} ->
        method == "POST" and endpoint == @issue <> "/comments"
      end)

    body
  end

  test "starting adds the running label and then removes the ready one", %{gh: gh, id: id} do
    assert :ok = Tickets.started(Workloads.get(id))

    assert StandInGitHub.writes(gh) == [
             {"POST", @issue <> "/labels", ["labels[]=heph:running"]},
             {"DELETE", @issue <> "/labels/heph%3Aready", []}
           ]
  end

  describe "ending" do
    test "failed leaves the failed label and a comment with the reason and the cost", %{
      gh: gh,
      id: id
    } do
      assert :ok = ended(id, "OUTCOME_FAILED", "gate test failed")

      assert StandInGitHub.writes(gh) |> Enum.map(fn {m, e, _} -> {m, e} end) == [
               {"POST", @issue <> "/labels"},
               {"DELETE", @issue <> "/labels/heph%3Arunning"},
               {"POST", @issue <> "/comments"}
             ]

      assert {"POST", _, ["labels[]=heph:failed"]} = hd(StandInGitHub.writes(gh))
      body = comment(gh)
      assert body =~ "failed"
      assert body =~ "gate test failed"
      assert body =~ "$0.126"
      assert body =~ "144096 tokens"
    end

    test "timed out is a failure too", %{gh: gh, id: id} do
      assert :ok = ended(id, "OUTCOME_TIMED_OUT", "the agent ran past the workload's timeout")

      assert {"POST", _, ["labels[]=heph:failed"]} = hd(StandInGitHub.writes(gh))
      assert comment(gh) =~ "timed out"
    end

    test "blocked leaves the blocked label and the question on the ticket", %{gh: gh, id: id} do
      {:ok, _} = Events.append(id, event(id, 1, started()))

      {:ok, _} =
        Events.append(
          id,
          event(id, 2, %{
            "finished" => %{
              "outcome" => "OUTCOME_BLOCKED",
              "reason" => "ambiguous",
              "question" => "Once or until it expires?"
            }
          })
        )

      Workloads.keep_record(id, %{
        "outcome" => "OUTCOME_BLOCKED",
        "question" => "Once or until it expires?"
      })

      assert :ok = Tickets.ended(Workloads.get(id))

      assert {"POST", _, ["labels[]=heph:blocked"]} = hd(StandInGitHub.writes(gh))
      body = comment(gh)
      assert body =~ "blocked"
      assert body =~ "Once or until it expires?"
      assert body =~ "ready"
    end

    test "cancelled leaves no label and says so", %{gh: gh, id: id} do
      assert :ok = ended(id, "OUTCOME_CANCELLED", "cancelled by a signal")

      writes = StandInGitHub.writes(gh)
      assert [{"DELETE", @issue <> "/labels/heph%3Arunning", []}, {"POST", _, _}] = writes
      assert comment(gh) =~ "cancelled"
    end

    test "completed names the pull request and leaves the running label", %{gh: gh, id: id} do
      assert :ok =
               ended(id, "OUTCOME_COMPLETED", "", %{
                 "pullRequest" => %{"url" => @pr, "number" => "52"}
               })

      writes = StandInGitHub.writes(gh)

      assert [{"POST", _, _}] = writes,
             "only the comment: the ticket is not done until the pull request is green"

      body = comment(gh)
      assert body =~ @pr
      assert body =~ "$0.126"
    end
  end

  test "a label or a comment that cannot be written does not change how the workload ended", %{
    gh: gh,
    id: id
  } do
    StandInGitHub.fail_write(gh, "POST", @issue <> "/labels", "gh: HTTP 403 Forbidden")
    StandInGitHub.fail_write(gh, "POST", @issue <> "/comments", "gh: HTTP 403 Forbidden")

    assert {:error, _} = ended(id, "OUTCOME_FAILED", "gate test failed")

    done = Workloads.get(id)
    assert done.state == "failed"
    assert done.reason == "gate test failed"
  end

  test "through the job: a run moves the ticket from ready, to running, to how it ended", %{
    gh: gh,
    id: id
  } do
    dir = Path.join(System.tmp_dir!(), "cp-tickets-run-#{System.unique_integer([:positive])}")

    on_exit(fn ->
      Application.delete_env(:control_plane, :runner)
      File.rm_rf(dir)
    end)

    events = [
      event(id, 1, started()),
      event(id, 2, workspace_ready()),
      event(id, 3, finished("OUTCOME_FAILED", "gate test failed"))
    ]

    executor = ControlPlane.StandIn.executor(Path.join(dir, "stand-in"), events: events)

    Application.put_env(:control_plane, :runner,
      executor: executor,
      runs_dir: Path.join(dir, "runs"),
      silence_limit_ms: 5_000,
      kill_grace_ms: 2_000
    )

    assert :ok = perform_job(ControlPlane.Jobs.RunWorkload, %{"workload_id" => id})

    writes = StandInGitHub.writes(gh)

    assert Enum.map(Enum.take(writes, 4), fn {method, endpoint, fields} ->
             {method, String.replace(endpoint, @issue, "#"), fields}
           end) == [
             {"POST", "#/labels", ["labels[]=heph:running"]},
             {"DELETE", "#/labels/heph%3Aready", []},
             {"POST", "#/labels", ["labels[]=heph:failed"]},
             {"DELETE", "#/labels/heph%3Arunning", []}
           ]

    assert [{"POST", @issue <> "/comments", [_body]}] = Enum.drop(writes, 4)
  end

  test "a slow label write does not lose an executor that has already exited", %{gh: gh, id: id} do
    # The stand-in gh takes a second; the executor is gone by the time it answers.
    File.write!(
      gh.gh,
      String.replace(File.read!(gh.gh), "echo \"$*\"", "sleep 1; echo \"$*\"", global: false)
    )

    dir = Path.join(System.tmp_dir!(), "cp-tickets-slow-#{System.unique_integer([:positive])}")

    on_exit(fn ->
      Application.delete_env(:control_plane, :runner)
      File.rm_rf(dir)
    end)

    events = [
      event(id, 1, started()),
      event(id, 2, finished("OUTCOME_FAILED", "gate test failed"))
    ]

    executor = ControlPlane.StandIn.executor(Path.join(dir, "stand-in"), events: events)

    Application.put_env(:control_plane, :runner,
      executor: executor,
      runs_dir: Path.join(dir, "runs"),
      silence_limit_ms: 5_000,
      kill_grace_ms: 2_000
    )

    assert :ok = perform_job(ControlPlane.Jobs.RunWorkload, %{"workload_id" => id})
    assert Workloads.get(id).state == "failed"
  end

  describe "a revision" do
    setup %{id: id} do
      revision = %{
        "pullRequestNumber" => "52",
        "pullRequestUrl" => @pr,
        "branch" => "heph/12-x",
        "attempt" => 1,
        "failures" => [%{"name" => "ci", "result" => "failure", "excerpt" => "x"}]
      }

      current = Workloads.get(id).workload

      ControlPlane.Repo.update_all(
        from(w in ControlPlane.Workload, where: w.id == ^id),
        set: [workload: Map.put(current, "revision", revision)]
      )

      :ok
    end

    test "starting does not touch the ticket's labels: it is already being worked on", %{
      gh: gh,
      id: id
    } do
      assert :ok = Tickets.started(Workloads.get(id))
      assert StandInGitHub.writes(gh) == []
    end

    test "completing says a revision was pushed, not that a pull request was opened", %{
      gh: gh,
      id: id
    } do
      assert :ok =
               ended(id, "OUTCOME_COMPLETED", "", %{
                 "pullRequest" => %{"url" => @pr, "number" => "52"}
               })

      body = comment(gh)
      assert body =~ "revision"
      assert body =~ @pr
      refute body =~ "opened a pull request"
    end

    test "a revision that failed ends the ticket failed like any workload", %{gh: gh, id: id} do
      assert :ok = ended(id, "OUTCOME_FAILED", "the agent changed nothing")

      assert {"POST", _, ["labels[]=heph:failed"]} = hd(StandInGitHub.writes(gh))
      assert comment(gh) =~ "the agent changed nothing"
    end
  end
end
