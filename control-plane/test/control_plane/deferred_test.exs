defmodule ControlPlane.DeferredTest do
  @moduledoc """
  A gate the workspace left to a check on the pull request (ADR 0028): the ticket is
  done only when that check ran and passed, and says what was left to it.
  """

  use ControlPlane.DataCase

  import Ecto.Query

  import ControlPlane.Fixtures

  alias ControlPlane.{Events, Settle, StandInGitHub, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @issue "repos/#{@repo}/issues/12"
  @pr_url "https://github.com/#{@repo}/pull/52"
  @sha "f6f2dc5f9df5fb32ff2a5c36ebee4e3bc9e159d5"
  @pull "repos/#{@repo}/pulls/52"
  @rules "repos/#{@repo}/rules/branches/main"
  @runs "repos/#{@repo}/commits/#{@sha}/check-runs?per_page=100"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-deferred-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous = Application.get_env(:control_plane, :intake)
    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [])

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous)
      File.rm_rf(dir)
    end)

    {:ok, record} = Workloads.create(workload())
    id = record.id
    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))

    # It ended just now, unless a test says otherwise.
    finished_minutes_ago(id, 0)

    # The workspace could not run `suite`, and left it to the check `ci / test`.
    Workloads.keep_record(id, %{
      "outcome" => "OUTCOME_COMPLETED",
      "pullRequest" => %{"url" => @pr_url},
      "gates" => [
        %{"name" => "lint", "result" => %{"gate" => "lint", "outcome" => "passed"}},
        %{
          "name" => "suite",
          "result" => %{
            "gate" => "suite",
            "outcome" => "deferred",
            "ci_check" => "ci / test",
            "reason" => "what it needs is not set: DATABASE_URL (environment variable)"
          }
        }
      ]
    })

    StandInGitHub.respond(gh, @pull, pull())
    StandInGitHub.respond(gh, @rules, [required(["lint"])])

    %{gh: gh, id: id}
  end

  defp pull,
    do: %{
      "state" => "open",
      "merged" => false,
      "head" => %{"sha" => @sha},
      "base" => %{"ref" => "main"}
    }

  defp required(contexts),
    do: %{
      "type" => "required_status_checks",
      "parameters" => %{"required_status_checks" => Enum.map(contexts, &%{"context" => &1})}
    }

  defp run(id, name, status, conclusion),
    do: %{"id" => id, "name" => name, "status" => status, "conclusion" => conclusion}

  defp checks(gh, list),
    do: StandInGitHub.respond(gh, @runs, %{"total_count" => length(list), "check_runs" => list})

  defp comments(gh),
    do: for({"POST", @issue <> "/comments", [body]} <- StandInGitHub.writes(gh), do: body)

  defp labels(gh),
    do: for({"POST", @issue <> "/labels", [field]} <- StandInGitHub.writes(gh), do: field)

  defp result(id), do: Enum.find(Settle.run(), &(&1.workload_id == id)).result

  # The workload ended this long ago.
  defp finished_minutes_ago(id, minutes) do
    at = DateTime.utc_now() |> DateTime.add(-minutes * 60, :second) |> DateTime.truncate(:second)

    ControlPlane.Repo.update_all(
      from(w in ControlPlane.Workload, where: w.id == ^id),
      set: [finished_at: at]
    )
  end

  test "the check it was left to passed: done, and the ticket says what was left and to what",
       %{gh: gh, id: id} do
    checks(gh, [
      run(1, "lint", "completed", "success"),
      run(2, "ci / test", "completed", "success")
    ])

    assert result(id) == :done

    assert labels(gh) == ["labels[]=heph:done"]
    assert [comment] = comments(gh)
    assert comment =~ "Left to CI"
    assert comment =~ "suite"
    assert comment =~ "ci / test"
    assert comment =~ "DATABASE_URL"
    refute comment =~ "lint"
  end

  test "the check it was left to failed: the ticket fails, naming the check", %{gh: gh, id: id} do
    checks(gh, [
      run(1, "lint", "completed", "success"),
      run(2, "ci / test", "completed", "failure")
    ])

    assert result(id) == {:failed, ["ci / test"]}

    assert labels(gh) == ["labels[]=heph:failed"]
    assert [comment] = comments(gh)
    assert comment =~ "ci / test"
  end

  test "the check it was left to has not appeared yet: it waits and writes nothing", %{
    gh: gh,
    id: id
  } do
    checks(gh, [run(1, "lint", "completed", "success")])

    assert {:waiting, message} = result(id)
    assert message =~ "ci / test"
    assert StandInGitHub.writes(gh) == []
    refute Workloads.get(id).settled_at
  end

  test "the check it was left to is still running: it waits", %{gh: gh, id: id} do
    checks(gh, [run(1, "lint", "completed", "success"), run(2, "ci / test", "in_progress", nil)])

    assert {:waiting, message} = result(id)
    assert message =~ "ci / test"
    assert StandInGitHub.writes(gh) == []
  end

  test "the check never appeared and every other check is done, long after the workload: it fails, saying so",
       %{gh: gh, id: id} do
    finished_minutes_ago(id, 30)
    checks(gh, [run(1, "lint", "completed", "success")])

    assert {:failed, ["ci / test"], _} = result(id)

    assert labels(gh) == ["labels[]=heph:failed"]
    assert [comment] = comments(gh)
    assert comment =~ "the gate `suite` was left to the check `ci / test`"
    assert comment =~ "never appeared"
    assert Workloads.get(id).settled_at
  end

  test "the check never appeared but another is still running: it waits however long it has been",
       %{gh: gh, id: id} do
    finished_minutes_ago(id, 30)
    checks(gh, [run(1, "lint", "in_progress", nil)])

    assert {:waiting, _} = result(id)
    assert StandInGitHub.writes(gh) == []
  end

  test "the check never appeared but it has only been a few minutes: it still waits", %{
    gh: gh,
    id: id
  } do
    finished_minutes_ago(id, 5)
    checks(gh, [run(1, "lint", "completed", "success")])

    assert {:waiting, _} = result(id)
    assert StandInGitHub.writes(gh) == []
  end

  test "a gate that passed in the workspace is not listed, and nothing left means nothing said",
       %{gh: gh, id: id} do
    Workloads.keep_record(id, %{
      "outcome" => "OUTCOME_COMPLETED",
      "pullRequest" => %{"url" => @pr_url},
      "gates" => [%{"name" => "lint", "result" => %{"gate" => "lint", "outcome" => "passed"}}]
    })

    checks(gh, [run(1, "lint", "completed", "success")])

    assert result(id) == :done
    assert [comment] = comments(gh)
    refute comment =~ "Left to CI"
  end

  test "a pull request that was already merged is done without being asked about the check",
       %{gh: gh, id: id} do
    StandInGitHub.respond(gh, @pull, Map.put(pull(), "merged", true))
    checks(gh, [])

    assert result(id) == :done
    assert [comment] = comments(gh)
    refute comment =~ "Left to CI"
  end

  test "a deferred check that failed is revised like any failing check", %{gh: gh, id: id} do
    previous = Application.get_env(:control_plane, :settle)
    on_exit(fn -> Application.put_env(:control_plane, :settle, previous) end)
    Application.put_env(:control_plane, :settle, revisions: 2)

    StandInGitHub.respond(
      gh,
      @pull,
      Map.put(pull(), "head", %{
        "sha" => @sha,
        "ref" => "heph/12-a-ticket",
        "repo" => %{"full_name" => @repo}
      })
    )

    checks(gh, [
      run(1, "lint", "completed", "success"),
      run(2, "ci / test", "completed", "failure")
    ])

    assert result(id) == {:revising, 1}

    revision = Enum.find(Workloads.for_ticket(@repo, 12), &(&1.parent_id == id))
    assert [%{"name" => "ci / test"}] = revision.workload["revision"]["failures"]
  end
end
