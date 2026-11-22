defmodule ControlPlane.BoundariesTest do
  @moduledoc """
  A repository whose tickets and code are in two systems that are not GitHub is settled
  through those two and nothing else: the tracker is read and written for the ticket, the
  code host for the change, each under its own name for the repository, and GitHub is never
  reached.
  """

  use ControlPlane.DataCase

  import ControlPlane.Fixtures

  alias ControlPlane.{Change, Check, Events, Settle, StandInGitHub, StandInSystems, Workloads}

  @repo "erinversfeldcodes/gazebolang"
  @sha "f6f2dc5f9df5fb32ff2a5c36ebee4e3bc9e159d5"

  setup do
    dir = Path.join(System.tmp_dir!(), "cp-boundaries-#{System.unique_integer([:positive])}")
    gh = StandInGitHub.install(dir)
    previous_intake = Application.get_env(:control_plane, :intake)
    previous_systems = Application.get_env(:control_plane, :systems)

    Application.put_env(:control_plane, :intake, gh: gh.gh, heph: gh.heph, repositories: [])

    Application.put_env(:control_plane, :systems, %{
      @repo => %{
        tracker: {ControlPlane.StandInTracker, "TRACK"},
        code_host: {ControlPlane.StandInCodeHost, "group/code"}
      }
    })

    start_supervised!(StandInSystems)

    on_exit(fn ->
      Application.put_env(:control_plane, :intake, previous_intake)

      if previous_systems,
        do: Application.put_env(:control_plane, :systems, previous_systems),
        else: Application.delete_env(:control_plane, :systems)

      File.rm_rf(dir)
    end)

    {:ok, record} = Workloads.create(workload())
    id = record.id
    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, finished("OUTCOME_COMPLETED", "")))

    Workloads.keep_record(id, %{
      "outcome" => "OUTCOME_COMPLETED",
      "pullRequest" => %{"url" => "https://github.com/#{@repo}/pull/52"}
    })

    StandInSystems.answer(:code_host, :change, {:ok, change()})
    StandInSystems.answer(:code_host, :required_checks, {:ok, ["ci"]})

    StandInSystems.answer(
      :code_host,
      :checks,
      {:ok, [%Check{id: 1, name: "ci", state: :completed, conclusion: "success"}]}
    )

    for operation <- [:add_label, :remove_label, :comment_on_ticket],
        do: StandInSystems.answer(:tracker, operation, :ok)

    %{gh: gh, id: id}
  end

  defp change do
    %Change{
      number: "52",
      state: :open,
      head_sha: @sha,
      head_ref: "heph/12-a-change",
      head_repository: "group/code",
      base_ref: "main",
      mergeability: :clean
    }
  end

  test "a ticket is settled by reading the code host and writing the tracker", %{gh: gh, id: id} do
    assert Enum.find(Settle.run(), &(&1.workload_id == id)).result == :done

    calls = StandInSystems.calls()

    # The change, its required checks and its checks are read from the code host, by the
    # name the repository has there.
    assert {:code_host, :change, ["group/code", "52"]} in calls
    assert {:code_host, :required_checks, ["group/code", "main"]} in calls
    assert {:code_host, :checks, ["group/code", @sha]} in calls

    # The ticket is written to the tracker, by the name the repository has there.
    assert {:tracker, :add_label, ["TRACK", 12, "heph:done"]} in calls
    assert {:tracker, :remove_label, ["TRACK", 12, "heph:running"]} in calls
    assert Enum.any?(calls, &match?({:tracker, :comment_on_ticket, ["TRACK", 12, _]}, &1))

    # Every call went to the side it belongs to: no read of the change reached the
    # tracker and no write of the ticket reached the code host.
    assert Enum.all?(calls, fn {side, operation, _} ->
             if side == :tracker,
               do: operation in [:add_label, :remove_label, :comment_on_ticket],
               else: operation in [:change, :required_checks, :checks]
           end)

    assert StandInGitHub.gh_calls(gh) == []
    assert Workloads.get(id).settled_at
  end
end
