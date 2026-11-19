defmodule ControlPlane.RecoveryTest do
  @moduledoc """
  A workload a stopped control plane left running is failed when the next one starts, and
  its ticket says so (#133). Without it the workload stays `running` for ever and its
  ticket `heph:running`.
  """

  use ControlPlane.DataCase

  import Ecto.Query

  import ControlPlane.Fixtures

  alias ControlPlane.{Events, Recovery, StandInGitHub, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @pr_comments "repos/#{@repo}/issues/52/comments"
  @pr_url "https://github.com/#{@repo}/pull/52"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-recovery-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)
    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [])

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      File.rm_rf(dir)
    end)

    %{gh: gh}
  end

  # A workload of ticket 12 in this state, last updated this long before now.
  defp workload_in(state, seconds_ago \\ 600, extra \\ %{}) do
    {:ok, record} = Workloads.create(Map.merge(workload(), extra))
    {:ok, _} = Events.append(record.id, event(record.id, 1, started()))

    at = DateTime.add(DateTime.utc_now(), -seconds_ago, :second)

    Repo.update_all(from(w in ControlPlane.Workload, where: w.id == ^record.id),
      set: [state: state, updated_at: at]
    )

    Workloads.get(record.id)
  end

  defp writes(gh), do: StandInGitHub.writes(gh)

  defp state(id), do: Workloads.get(id).state

  # The moment the control plane that is starting began.
  defp boot, do: DateTime.utc_now()

  test "workloads that were preparing, running or gating are failed, saying why" do
    lost = for state <- ["preparing", "running", "gating"], do: workload_in(state)

    results = Recovery.run(boot())

    assert length(results) == 3

    for workload <- lost do
      recovered = Workloads.get(workload.id)
      assert recovered.state == "failed"
      assert recovered.reason =~ "control plane stopped"
      assert recovered.reason =~ "lost"
      assert recovered.finished_at
    end
  end

  test "the states that are not in flight are left as they are" do
    others =
      for state <- ["queued", "completed", "failed", "blocked", "cancelled", "timed_out"],
          do: {state, workload_in(state)}

    assert Recovery.run(boot()) == []

    for {state, workload} <- others, do: assert(state(workload.id) == state)
  end

  test "the ticket gets the failed label, loses the running one, and a comment", %{gh: gh} do
    workload_in("running")

    assert [%{result: :ok}] = Recovery.run(boot())

    posted = for {"POST", @issue <> "/labels", [field]} <- writes(gh), do: field
    removed = for {"DELETE", @issue <> "/labels/" <> label, _} <- writes(gh), do: label

    assert posted == ["labels[]=heph:failed"]
    assert removed == ["heph%3Arunning"]

    assert [{"POST", @issue <> "/comments", ["body=" <> body]}] =
             Enum.filter(writes(gh), fn {m, e, _} ->
               m == "POST" and e == @issue <> "/comments"
             end)

    assert body =~ "control plane stopped"
  end

  test "a workload that started after this control plane did is left alone" do
    just_now = workload_in("running", 0)

    # The control plane began a minute ago: this workload was updated after that.
    assert Recovery.run(DateTime.add(DateTime.utc_now(), -60, :second)) == []

    assert state(just_now.id) == "running"
  end

  test "running it twice changes nothing the second time", %{gh: gh} do
    workload_in("running")

    assert [_] = Recovery.run(boot())
    written = length(writes(gh))

    assert Recovery.run(boot()) == []
    assert length(writes(gh)) == written
  end

  test "a review that was lost says so on its pull request, once, and not on the ticket", %{
    gh: gh
  } do
    {:ok, parent} = Workloads.create(workload())

    review =
      parent.workload
      |> Map.put("id", Ecto.UUID.generate())
      |> Map.put("review", %{
        "pullRequestNumber" => "52",
        "pullRequestUrl" => @pr_url,
        "lens" => "rust",
        "lensDocument" => "x"
      })

    {:ok, record} = Workloads.create(review, parent: parent.id)

    Repo.update_all(from(w in ControlPlane.Workload, where: w.id == ^record.id),
      set: [state: "running", updated_at: DateTime.add(DateTime.utc_now(), -600, :second)]
    )

    assert [_] = Recovery.run(boot())

    assert state(record.id) == "failed"

    comments =
      for {"POST", @pr_comments, ["body=" <> body]} <- writes(gh), do: body

    assert [body] = comments
    assert body =~ "could not review"

    for {_method, endpoint, _} <- writes(gh), do: refute(endpoint =~ "issues/12")
  end

  test "a revision that was lost tells the ticket and the pull request", %{gh: gh} do
    revision = %{
      "revision" => %{
        "pullRequestNumber" => "52",
        "pullRequestUrl" => @pr_url,
        "branch" => "heph/12-x",
        "attempt" => 1,
        "failures" => []
      }
    }

    workload_in("running", 600, revision)

    assert [_] = Recovery.run(boot())

    assert Enum.any?(writes(gh), fn {m, e, _} -> m == "POST" and e == @issue <> "/comments" end)
    assert Enum.any?(writes(gh), fn {m, e, _} -> m == "POST" and e == @pr_comments end)
  end

  test "a write to GitHub that fails does not stop the others being recovered", %{gh: gh} do
    first = workload_in("running")
    second = workload_in("gating")
    StandInGitHub.fail_write(gh, "POST", @issue <> "/labels", "gh: HTTP 403 Forbidden")

    results = Recovery.run(boot())

    assert length(results) == 2
    assert Enum.all?(results, &match?({:error, _}, &1.result))
    assert state(first.id) == "failed"
    assert state(second.id) == "failed"
  end

  test "it is part of starting the application, except in the test suite" do
    refute Application.get_env(:control_plane, :recover_on_start)
    assert ControlPlane.Application.recover_on_start?(nil)
    refute ControlPlane.Application.recover_on_start?(false)
  end
end
