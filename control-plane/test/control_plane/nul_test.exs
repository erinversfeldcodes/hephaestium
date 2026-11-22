defmodule ControlPlane.NulTest do
  @moduledoc """
  Postgres cannot keep a NUL byte in text or in jsonb, so text from an agent or a tool that holds
  one would raise in the process that keeps it. Text with a NUL is kept with each NUL replaced
  by U+FFFD, so the stream has no gap and nothing raises, and the raw line stays in the run's own
  events file.
  """

  use ControlPlane.DataCase

  import ControlPlane.Fixtures

  alias ControlPlane.{Clean, Events, Workloads}

  @nul <<0>>
  @replacement "\u{FFFD}"

  setup do
    {:ok, created} = Workloads.create(workload())
    id = created.id
    {:ok, _} = Events.append(id, event(id, 1, started()))
    %{id: id}
  end

  describe "an event with a NUL byte" do
    test "in an agent message is kept, cleaned, and the stream goes on", %{id: id} do
      text = "a binary file: ab" <> @nul <> "cd"

      assert {:ok, kept} = Events.append(id, event(id, 2, agent_message(text)))

      assert kept.event["agentMessage"]["text"] == "a binary file: ab" <> @replacement <> "cd"
      assert {:ok, _} = Events.append(id, event(id, 3, agent_message("and on")))
      assert Enum.map(Events.list(id), & &1.sequence) == [1, 2, 3], "no gap"
    end

    test "in a tool result or a gate's result, however deep, is kept, cleaned", %{id: id} do
      result = %{"toolResult" => %{"callId" => "t1", "output" => "out" <> @nul <> "put"}}
      assert {:ok, kept} = Events.append(id, event(id, 2, result))
      assert kept.event["toolResult"]["output"] == "out" <> @replacement <> "put"

      gate =
        %{
          "gateFinished" => %{
            "name" => "test",
            "result" => %{"command" => ["sh", "-c", "echo" <> @nul], "outcome" => "failed"}
          }
        }

      assert {:ok, kept} = Events.append(id, event(id, 3, gate))

      assert kept.event["gateFinished"]["result"]["command"] == [
               "sh",
               "-c",
               "echo" <> @replacement
             ]
    end

    test "changes nothing else about the event, and leaves a clean one as it was", %{id: id} do
      clean = event(id, 2, agent_message("just words, 日本語, and a number 3"))

      assert {:ok, kept} = Events.append(id, clean)

      assert kept.event == clean
    end
  end

  describe "a record or a reason with a NUL byte" do
    test "a record is kept, cleaned", %{id: id} do
      record = %{
        "outcome" => "OUTCOME_FAILED",
        "reason" => "the agent printed " <> @nul <> " and stopped",
        "costs" => [%{"unit" => "COST_UNIT_USD", "amount" => 0.5}]
      }

      assert :ok = Workloads.keep_record(id, record)

      kept = Workloads.get(id).record
      assert kept["reason"] == "the agent printed " <> @replacement <> " and stopped"
      assert kept["costs"] == [%{"unit" => "COST_UNIT_USD", "amount" => 0.5}]
    end

    test "a workload that failed with a reason that has one is failed, with it cleaned", %{id: id} do
      assert Workloads.fail(id, "exited with " <> @nul <> "output") == :ok

      failed = Workloads.get(id)
      assert failed.state == "failed"
      assert failed.reason == "exited with " <> @replacement <> "output"
    end
  end

  describe "Clean.deep/1" do
    test "cleans every string in a map or a list and leaves what is not a string alone" do
      cleaned =
        Clean.deep(%{
          "a" => "x" <> @nul,
          "b" => [1, 2.5, true, nil, "y" <> @nul],
          "c" => %{"d" => "z" <> @nul <> @nul}
        })

      assert cleaned == %{
               "a" => "x" <> @replacement,
               "b" => [1, 2.5, true, nil, "y" <> @replacement],
               "c" => %{"d" => "z" <> @replacement <> @replacement}
             }
    end

    test "does not look inside a struct, which is a value and not a map of strings" do
      time = ~U[2026-11-22 07:00:00Z]

      assert Clean.deep(%{"at" => time}) == %{"at" => time}
    end
  end
end
