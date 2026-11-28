defmodule ControlPlane.EarlierAttempts do
  @moduledoc """
  Why the ticket's earlier attempts ended, for the request of a new implementation.

  Like a person's answers (`ControlPlane.Answers`), it is read from the ticket's earlier
  workloads when the request is made and kept nowhere else. An attempt counts when it failed,
  timed out, or completed and was then refused by a lens (settle keeps the findings as that
  workload's reason). A completed attempt that nobody refused says nothing, and a first attempt
  has none, so its request is unchanged.
  """

  alias ControlPlane.Workloads

  @attempts 3
  @total 8_000
  # Three attempts share the total, so a single long gate output cannot push the others out
  # of it: each may use a third of it, and the total is checked again after the headings.
  @each div(@total, @attempts)

  @doc "Put the latest earlier attempts that did not end well in the request, when there are any."
  @spec add(map(), String.t(), integer()) :: map()
  def add(workload, repository, number) do
    attempts =
      repository
      |> Workloads.for_ticket(number)
      |> Enum.reverse()
      |> Enum.flat_map(&attempt/1)
      |> Enum.take(@attempts)
      |> within_total()

    case attempts do
      [] -> workload
      _ -> Map.put(workload, "earlierAttempts", attempts)
    end
  end

  defp attempt(%{state: "failed"} = workload) do
    case failed_gate(workload.record) do
      nil -> [item("failed", workload.reason)]
      gate -> [item("a failed gate: #{gate["name"]}", gate_text(gate, workload.reason))]
    end
  end

  defp attempt(%{state: "timed_out"} = workload),
    do: [item("timed out in #{step(workload.record)}", workload.reason)]

  defp attempt(%{state: "completed", reason: reason}) when is_binary(reason) and reason != "",
    do: [item("refused by the review lenses", reason)]

  defp attempt(_workload), do: []

  defp item(ended, why), do: %{"ended" => ended, "why" => tail(to_string(why || ""), @each)}

  defp failed_gate(%{"gates" => gates}) when is_list(gates) do
    Enum.find(gates, fn
      %{"name" => name, "result" => %{"outcome" => outcome}} when is_binary(name) ->
        outcome in ["failed", "timed out", "timed_out"]

      _ ->
        false
    end)
  end

  defp failed_gate(_record), do: nil

  defp gate_text(gate, reason) do
    case gate["output"] do
      output when is_binary(output) and output != "" ->
        "#{gate["result"]["outcome"]}:\n" <> output

      _ ->
        to_string(reason || gate["result"]["outcome"])
    end
  end

  # The last step the run began is the one it was in when time ran out.
  defp step(%{"steps" => [_ | _] = steps}) do
    steps |> List.last() |> step_name()
  end

  defp step(_record), do: "a step the record does not name"

  defp step_name(step) do
    kind =
      case step["kind"] do
        "KIND_REVIEW" -> "review"
        "KIND_REVISION" -> "revision"
        "KIND_IMPLEMENTATION" -> "implementation"
        _ -> "an unnamed step"
      end

    [kind, step["lens"] && "lens #{step["lens"]}", step["round"] && "round #{step["round"]}"]
    |> Enum.filter(&(&1 not in [nil, false, ""]))
    |> Enum.join(", ")
  end

  # The end of a text is what says why, so it is the end that is kept, never splitting a
  # character, and the text says what was cut.
  defp tail(text, max) when byte_size(text) <= max, do: text

  defp tail(text, max) do
    notice = "[#{byte_size(text)} bytes, the start cut]\n"
    keep = max(max - byte_size(notice), 0)
    kept = binary_part(text, byte_size(text) - keep, keep) |> drop_partial_character()
    notice <> kept
  end

  defp drop_partial_character(<<byte, rest::binary>>) when byte in 0x80..0xBF,
    do: drop_partial_character(rest)

  defp drop_partial_character(text), do: text

  # The headings and cut notes add to the texts, so the whole is cut again to the total.
  defp within_total(attempts) do
    {kept, _} =
      Enum.map_reduce(attempts, @total, fn %{"ended" => ended, "why" => why} = a, left ->
        room = max(left - byte_size(ended), 0)
        why = if byte_size(why) <= room, do: why, else: tail(why, room)
        {%{a | "why" => why}, left - byte_size(ended) - byte_size(why)}
      end)

    kept
  end
end
