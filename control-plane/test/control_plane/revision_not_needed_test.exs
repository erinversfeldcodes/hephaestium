defmodule ControlPlane.RevisionNotNeededTest do
  @moduledoc """
  A revision whose pull request merged or closed before it ran, or while it ran, has nothing
  to revise: it ends as not needed, the ticket keeps the label the merge gave it and gets no
  comment. A revision of an open pull request is run and reported as before.

  The code host is the stand-in `gh`, which answers in the shapes GitHub gives; that shows the
  control plane's decisions, not that GitHub answers so.
  """

  use ControlPlane.DataCase
  use Oban.Testing, repo: ControlPlane.Repo

  import Ecto.Query
  import ControlPlane.Fixtures

  alias ControlPlane.Jobs.RunWorkload

  alias ControlPlane.{
    DeviceTokens,
    Events,
    Leases,
    StandIn,
    StandInGitHub,
    Tickets,
    Workload,
    WorkloadLease,
    Workloads
  }

  @repo "erinversfeldcodes/gazebolang"
  @pull "repos/#{@repo}/pulls/52"
  @pr "https://github.com/erinversfeldcodes/gazebolang/pull/52"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-notneeded-#{System.unique_integer([:positive])}")
    stand_in = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)

    Application.put_env(:control_plane, :intake,
      gh: stand_in.gh,
      heph: stand_in.heph,
      repositories: []
    )

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      Application.delete_env(:control_plane, :runner)
      File.rm_rf(dir)
    end)

    {:ok, record} = Workloads.create(workload())

    ControlPlane.Repo.update_all(
      from(w in Workload, where: w.id == ^record.id),
      set: [
        workload:
          Map.put(record.workload, "revision", %{
            "pullRequestNumber" => "52",
            "pullRequestUrl" => @pr,
            "branch" => "heph/12-x",
            "attempt" => 1,
            "failures" => []
          })
      ]
    )

    %{gh: stand_in, id: record.id, dir: dir}
  end

  defp pull(gh, state, merged),
    do: StandInGitHub.respond(gh, @pull, %{"state" => state, "merged" => merged, "head" => %{}})

  defp ticket_writes(gh), do: StandInGitHub.writes(gh)

  # The executor that could not check out the base commit, as it ends.
  defp failed_checkout(id) do
    {:ok, _} = Events.append(id, event(id, 1, started()))

    {:ok, _} =
      Events.append(
        id,
        event(
          id,
          2,
          finished(
            "OUTCOME_FAILED",
            "could not check out the base commit in the clone: git exited with code 128"
          )
        )
      )

    Workloads.keep_record(id, %{"outcome" => "OUTCOME_FAILED"})
  end

  defp configure(dir, id) do
    executor =
      StandIn.executor(Path.join(dir, "stand-in"),
        events: [event(id, 1, started()), event(id, 2, finished())],
        record: %{"outcome" => "OUTCOME_COMPLETED"}
      )

    Application.put_env(:control_plane, :runner,
      executor: executor,
      runs_dir: Path.join(dir, "runs"),
      silence_limit_ms: 5_000,
      kill_grace_ms: 2_000
    )
  end

  describe "before the revision starts" do
    test "a revision whose pull request merged is not run, and the ticket is untouched", %{
      gh: gh,
      id: id,
      dir: dir
    } do
      pull(gh, "closed", true)
      configure(dir, id)

      assert :ok = perform_job(RunWorkload, %{"workload_id" => id}, [])

      ended = Workloads.get(id)
      assert ended.state == "not_needed"
      assert ended.reason =~ "merged"
      assert Events.list(id) == [], "the executor was never started"
      assert ticket_writes(gh) == [], "no label changed and nothing was posted"
    end

    test "a revision whose pull request was closed is not run either", %{
      gh: gh,
      id: id,
      dir: dir
    } do
      pull(gh, "closed", false)
      configure(dir, id)

      assert :ok = perform_job(RunWorkload, %{"workload_id" => id}, [])
      assert Workloads.get(id).state == "not_needed"
      assert ticket_writes(gh) == []
    end

    test "a revision of an open pull request is run as before", %{gh: gh, id: id, dir: dir} do
      pull(gh, "open", false)
      configure(dir, id)

      assert :ok = perform_job(RunWorkload, %{"workload_id" => id}, [])
      assert Workloads.get(id).state == "completed"
      assert length(Events.list(id)) == 2
    end
  end

  describe "when the revision has failed" do
    test "a revision that could not check out a merged pull request is not failed", %{
      gh: gh,
      id: id
    } do
      pull(gh, "closed", true)
      failed_checkout(id)

      assert :ok = Tickets.ended(Workloads.get(id))

      assert Workloads.get(id).state == "not_needed"
      assert ticket_writes(gh) == [], "no heph:failed label and no comment"
    end

    test "a revision of an open pull request that failed is reported as it was", %{
      gh: gh,
      id: id
    } do
      pull(gh, "open", false)
      failed_checkout(id)

      assert :ok = Tickets.ended(Workloads.get(id))

      assert Workloads.get(id).state == "failed"
      assert {"POST", _, ["labels[]=heph:failed"]} = hd(ticket_writes(gh))
    end

    test "when the code host cannot say, the failure stands", %{gh: gh, id: id} do
      StandInGitHub.fail(gh, @pull, "gh: Bad gateway (HTTP 502)")
      failed_checkout(id)

      assert :ok = Tickets.ended(Workloads.get(id))
      assert Workloads.get(id).state == "failed"
    end
  end

  describe "when an executor on someone's machine claims" do
    setup %{id: id} do
      {:ok, %{record: token}} = DeviceTokens.issue("erinversfeldcodes", "laptop")
      approve(id)

      # The oldest, so it is the one a claim reaches first.
      ControlPlane.Repo.update_all(
        from(w in Workload, where: w.id == ^id),
        set: [inserted_at: DateTime.add(DateTime.utc_now(), -3600, :second)]
      )

      %{token: token}
    end

    defp approve(id) do
      %Workload{workload: body} = Workloads.get(id)

      ControlPlane.Repo.update_all(
        from(w in Workload, where: w.id == ^id),
        set: [workload: put_in(body, ["approval", "approvedBy"], "erinversfeldcodes")]
      )
    end

    defp queue_another do
      {:ok, other} =
        workload()
        |> put_in(["approval", "approvedBy"], "erinversfeldcodes")
        |> Workloads.create()

      other.id
    end

    test "a revision of a merged pull request is let go and the next workload is given", %{
      gh: gh,
      id: id,
      token: token
    } do
      pull(gh, "closed", true)
      other = queue_another()

      assert {:ok, %Workload{id: ^other}} = Leases.claim(token)

      assert Workloads.get(id).state == "not_needed"
      refute ControlPlane.Repo.get(WorkloadLease, id).active

      # Only the workload that was given marks its own ticket as running; the revision's
      # ticket is not failed and gets no comment.
      for {_method, endpoint, fields} <- ticket_writes(gh) do
        refute endpoint =~ "comments"
        refute "labels[]=heph:failed" in fields
      end
    end

    test "when it is the only workload, nothing is given", %{gh: gh, id: id, token: token} do
      pull(gh, "closed", true)

      assert :none = Leases.claim(token)

      assert Workloads.get(id).state == "not_needed"
      refute ControlPlane.Repo.get(WorkloadLease, id).active
      assert ticket_writes(gh) == []
    end

    test "a revision of an open pull request is given as before", %{gh: gh, id: id, token: token} do
      pull(gh, "open", false)

      assert {:ok, %Workload{id: ^id}} = Leases.claim(token)
      assert ControlPlane.Repo.get(WorkloadLease, id).active
    end
  end
end
