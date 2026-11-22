defmodule ControlPlane.StepEventsTest do
  @moduledoc """
  The executor says which agent of a run that reads its own change is running (ADR 0035): a
  step starts and ends. The control plane keeps them with the rest of the stream, and they change
  nothing about what state the workload is in.
  """

  use ControlPlane.DataCase

  import ControlPlane.Fixtures

  alias ControlPlane.{Events, Workloads}

  defp step_started(id, sequence, kind, round, lens),
    do:
      event(id, sequence, %{
        "stepStarted" => %{"kind" => kind, "round" => round, "lens" => lens}
      })

  defp step_finished(id, sequence, verdict),
    do:
      event(id, sequence, %{
        "stepFinished" => %{
          "kind" => "KIND_REVIEW",
          "round" => 1,
          "lens" => "rust",
          "verdict" => verdict
        }
      })

  test "are kept in order with the rest of the stream and change the workload's state in no way" do
    {:ok, created} = Workloads.create(workload())
    id = created.id

    {:ok, _} = Events.append(id, event(id, 1, started()))
    {:ok, _} = Events.append(id, event(id, 2, workspace_ready()))
    assert Workloads.get(id).state == "running"

    assert {:ok, kept} = Events.append(id, step_started(id, 3, "KIND_REVIEW", 1, "rust"))
    assert kept.kind == "stepStarted"
    assert Workloads.get(id).state == "running", "a step starting is not the workload moving on"

    {:ok, _} = Events.append(id, event(id, 4, agent_message("reading")))
    assert {:ok, ended} = Events.append(id, step_finished(id, 5, "needs_revision"))
    assert ended.kind == "stepFinished"

    assert Enum.map(Events.list(id), & &1.kind) ==
             ["started", "workspaceReady", "stepStarted", "agentMessage", "stepFinished"]

    assert Workloads.get(id).state == "running"
  end

  test "keep the gaps in a stream refused as they are for any other event" do
    {:ok, created} = Workloads.create(workload())
    id = created.id
    {:ok, _} = Events.append(id, event(id, 1, started()))

    assert {:error, :gap_in_sequence} =
             Events.append(id, step_started(id, 3, "KIND_REVIEW", 1, "rust"))
  end
end
