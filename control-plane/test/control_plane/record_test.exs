defmodule ControlPlane.RecordTest do
  use ControlPlane.DataCase

  import ControlPlane.Fixtures

  alias ControlPlane.{Events, Workloads}

  defp created(id \\ Ecto.UUID.generate()) do
    {:ok, workload} = Workloads.create(workload(id))
    workload
  end

  defp append!(id, sequence, kind) do
    {:ok, event} = Events.append(id, event(id, sequence, kind))
    event
  end

  describe "a workload" do
    test "is created from its JSON and starts queued" do
      id = Ecto.UUID.generate()

      assert {:ok, record} = Workloads.create(workload(id))

      assert record.id == id
      assert record.repository == "erinversfeldcodes/gazebolang"
      assert record.ticket_number == 12
      assert record.state == "queued"
      assert Workloads.get(id).workload == workload(id), "the workload is kept as it was given"
    end

    test "is not created twice, or from something that is not a workload" do
      workload = created()

      assert {:error, :already_exists} = Workloads.create(workload(workload.id))
      assert {:error, :not_a_workload} = Workloads.create(%{"id" => Ecto.UUID.generate()})
      assert {:error, :not_a_workload} = Workloads.create(Map.put(workload(), "id", "not-a-uuid"))

      assert {:error, :not_a_workload} =
               Workloads.create(put_in(workload(), ["ticket", "number"], "12x"))

      assert {:error, :not_a_workload} =
               Workloads.create(put_in(workload(), ["ticket", "number"], "0"))
    end
  end

  describe "a stream of events" do
    test "is kept in order, as given, and the state follows it to its end" do
      id = created().id

      stream = [
        {1, started(), "preparing"},
        {2, workspace_ready(), "running"},
        {3, agent_message(), "running"},
        {4, gate_started(), "gating"},
        {5, gate_finished(), "gating"},
        {6, finished("OUTCOME_COMPLETED"), "completed"}
      ]

      for {sequence, kind, state} <- stream do
        append!(id, sequence, kind)
        assert Workloads.get(id).state == state, "after event #{sequence}"
      end

      assert Enum.map(Events.list(id), & &1.sequence) == [1, 2, 3, 4, 5, 6]

      assert Enum.map(Events.list(id), & &1.kind) ==
               ~w(started workspaceReady agentMessage gateStarted gateFinished finished)

      assert Enum.map(Events.list(id), & &1.event) ==
               Enum.map(stream, fn {sequence, kind, _} -> event(id, sequence, kind) end),
             "each event is kept whole, as the contract wrote it"

      done = Workloads.get(id)
      assert done.started_at == ~U[2026-11-18 07:00:01.000000Z]
      assert done.finished_at == ~U[2026-11-18 07:00:06.000000Z]
    end

    test "ends the workload with the outcome the finished event carries" do
      for {outcome, state, reason} <- [
            {"OUTCOME_COMPLETED", "completed", ""},
            {"OUTCOME_FAILED", "failed", "gate test failed"},
            {"OUTCOME_BLOCKED", "blocked", "needs an answer"},
            {"OUTCOME_TIMED_OUT", "timed_out", "too long"},
            {"OUTCOME_CANCELLED", "cancelled", "cancelled by a signal"}
          ] do
        id = created().id
        append!(id, 1, started())
        append!(id, 2, finished(outcome, reason))

        done = Workloads.get(id)
        assert done.state == state, outcome
        assert done.reason == reason, outcome
      end
    end
  end

  describe "an event that breaks a rule of the stream" do
    setup do
      id = created().id
      append!(id, 1, started())
      append!(id, 2, workspace_ready())
      %{id: id}
    end

    test "that repeats a sequence is refused and the stream is unchanged", %{id: id} do
      assert {:error, :repeated_sequence} = Events.append(id, event(id, 2, agent_message()))
      assert {:error, :repeated_sequence} = Events.append(id, event(id, 1, agent_message()))
      assert length(Events.list(id)) == 2
    end

    test "that leaves a gap is refused", %{id: id} do
      assert {:error, :gap_in_sequence} = Events.append(id, event(id, 4, agent_message()))
      assert length(Events.list(id)) == 2
    end

    test "that goes back in time is refused", %{id: id} do
      earlier = ~U[2026-11-18 06:00:00Z]
      assert {:error, :time_goes_back} = Events.append(id, event(id, 3, agent_message(), earlier))
      assert length(Events.list(id)) == 2
    end

    test "that comes after finished is refused", %{id: id} do
      append!(id, 3, finished())
      assert {:error, :after_finished} = Events.append(id, event(id, 4, agent_message()))
      assert length(Events.list(id)) == 3
    end

    test "that belongs to no workload, or another, is refused", %{id: id} do
      stranger = Ecto.UUID.generate()
      assert {:error, :unknown_workload} = Events.append(stranger, event(stranger, 1, started()))
      assert {:error, :wrong_workload} = Events.append(id, event(stranger, 3, agent_message()))
    end

    test "that is not an event is refused", %{id: id} do
      good = event(id, 3, agent_message())
      no_kind = Map.drop(good, ["agentMessage"])
      two_kinds = Map.merge(good, gate_started())

      assert {:error, :not_an_event} = Events.append(id, no_kind)
      assert {:error, :not_an_event} = Events.append(id, two_kinds)
      assert {:error, :not_an_event} = Events.append(id, Map.put(good, "sequence", "three"))
      assert {:error, :not_an_event} = Events.append(id, Map.put(good, "sequence", "3x"))
      assert {:error, :not_an_event} = Events.append(id, Map.put(good, "sequence", "0"))
      assert {:error, :not_an_event} = Events.append(id, event(id, 3, finished("OUTCOME_DONE")))
      assert {:error, :not_an_event} = Events.append(id, Map.put(good, "time", "later"))
      assert length(Events.list(id)) == 2
    end

    test "the first event must be the first in the stream" do
      id = created().id
      assert {:error, :gap_in_sequence} = Events.append(id, event(id, 2, started()))
    end
  end

  test "an event cannot be changed or deleted afterwards, whatever asks" do
    id = created().id
    append!(id, 1, started())

    assert {:error, %Postgrex.Error{postgres: %{message: update_message}}} =
             Repo.query("UPDATE events SET kind = 'x' WHERE workload_id = $1", [
               Ecto.UUID.dump!(id)
             ])

    assert {:error, %Postgrex.Error{postgres: %{message: delete_message}}} =
             Repo.query("DELETE FROM events WHERE workload_id = $1", [Ecto.UUID.dump!(id)])

    assert update_message =~ "append-only"
    assert delete_message =~ "append-only"
    assert [%{kind: "started"}] = Events.list(id)
  end
end
