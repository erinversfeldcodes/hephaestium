defmodule ControlPlane.Fixtures do
  @moduledoc """
  Workloads and events as the contracts write them: the workload is the
  contract's own valid case, and an event is the Protobuf JSON mapping, with its
  sequence as a string.
  """

  @workload_case Path.expand(
                   "../../../cases/runtime/Workload/valid/claude-code-subscription.json",
                   __DIR__
                 )

  @doc "The contract's valid workload, with an id of its own unless one is given."
  def workload(id \\ Ecto.UUID.generate()) do
    @workload_case |> File.read!() |> Jason.decode!() |> Map.put("id", id)
  end

  @base ~U[2026-11-18 07:00:00Z]

  @doc "An event of the workload, the given number in its stream, of the given kind."
  def event(workload_id, sequence, kind, time \\ nil) do
    time = time || DateTime.add(@base, sequence, :second)

    Map.merge(
      %{
        "workloadId" => workload_id,
        "sequence" => Integer.to_string(sequence),
        "time" => DateTime.to_iso8601(time)
      },
      kind
    )
  end

  def started, do: %{"started" => %{"executorVersion" => "0.0.0"}}

  def workspace_ready,
    do: %{"workspaceReady" => %{"path" => "/work/w", "commit" => String.duplicate("a", 40)}}

  def agent_message(text \\ "hello"), do: %{"agentMessage" => %{"text" => text}}
  def gate_started(name \\ "format"), do: %{"gateStarted" => %{"name" => name}}

  def gate_finished(name \\ "format"),
    do: %{"gateFinished" => %{"name" => name, "result" => %{"outcome" => "passed"}}}

  def finished(outcome \\ "OUTCOME_COMPLETED", reason \\ ""),
    do: %{"finished" => %{"outcome" => outcome, "reason" => reason}}
end
