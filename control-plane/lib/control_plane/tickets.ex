defmodule ControlPlane.Tickets do
  @moduledoc """
  What a ticket shows of its workload: its labels move as the workload progresses,
  and a comment says how it ended.

  The label `heph:ready` is the approval, so a workload that has started spends it.
  A workload that completed has opened a pull request, and the ticket is not done
  until that pull request's required checks are green, so its label does not
  change here. Writing to GitHub can fail, and that never changes how a workload
  ended: it is logged, and what could not be written is returned.
  """

  require Logger

  alias ControlPlane.{GitHub, Workload}

  @ready "heph:ready"
  @running "heph:running"
  @failed "heph:failed"
  @blocked "heph:blocked"

  @doc """
  The workload has started: add the running label and then remove the ready one,
  in that order, so the ticket is never without a state.
  """
  @spec started(%Workload{}) :: :ok | {:error, String.t()}
  def started(%Workload{workload: %{"revision" => _}}), do: :ok

  def started(workload) do
    first_error([
      add_label(workload, @running),
      remove_label(workload, @ready)
    ])
  end

  @doc "The workload has ended: leave the label that says how, and a comment."
  @spec ended(%Workload{}) :: :ok | {:error, String.t()}
  def ended(%Workload{state: state} = workload) when state in ["failed", "timed_out"] do
    first_error([
      add_label(workload, @failed),
      remove_label(workload, @running),
      comment(workload)
    ])
  end

  def ended(%Workload{state: "blocked"} = workload) do
    first_error([
      add_label(workload, @blocked),
      remove_label(workload, @running),
      comment(workload)
    ])
  end

  def ended(%Workload{state: "cancelled"} = workload) do
    first_error([remove_label(workload, @running), comment(workload)])
  end

  def ended(%Workload{state: "completed"} = workload), do: first_error([comment(workload)])

  def ended(_workload), do: :ok

  @done "heph:done"

  @doc """
  The workload's pull request has been looked at and the ticket settled: `:done` when
  its required checks are green (or it was merged), `{:failed, names}` when some
  are not, `:abandoned` when it was closed without merging.
  """
  @spec settled(%Workload{}, :done | {:failed, [String.t()]} | :abandoned) ::
          :ok | {:error, String.t()}
  def settled(workload, :done) do
    first_error([
      add_label(workload, @done),
      remove_label(workload, @running),
      note(workload, "the pull request's required checks are green. The ticket is done.")
    ])
  end

  def settled(workload, {:failed, names}), do: settled(workload, {:failed, names, nil})

  def settled(workload, {:failed, names, extra}) do
    first_error([
      add_label(workload, @failed),
      remove_label(workload, @running),
      note(
        workload,
        "the pull request's required checks did not pass: #{Enum.join(names, ", ")}." <>
          reran(workload) <> if(extra, do: "\n\n" <> extra, else: "")
      )
    ])
  end

  def settled(workload, :abandoned) do
    first_error([
      remove_label(workload, @running),
      note(workload, "the pull request was closed without being merged.")
    ])
  end

  @doc """
  The pull request's checks really failed and a revision has been started: the ticket
  says which, and that the agent will investigate on the same branch. The labels do
  not change: the ticket is still being worked on.
  """
  @spec revising(%Workload{}, [String.t()], pos_integer(), pos_integer()) ::
          :ok | {:error, String.t()}
  def revising(workload, names, attempt, max) do
    note(
      workload,
      "the pull request's checks did not pass: #{Enum.join(names, ", ")}. Starting revision #{attempt} of #{max}: the agent will investigate and push a fix to the same branch."
    )
  end

  # Which cancelled checks were re-run first, if any, and how often.
  defp reran(%{reruns: reruns}) when is_map(reruns) and map_size(reruns) > 0 do
    times = reruns |> Map.values() |> Enum.max()
    " Cancelled checks were re-run #{times} times first."
  end

  defp reran(_), do: ""

  defp note(workload, text) do
    url = get_in(workload.record || %{}, ["pullRequest", "url"])

    write("POST", issue(workload) <> "/comments", [
      "body=**Hephaestium:** #{text}\n\nPull request: #{url}"
    ])
  end

  defp issue(workload), do: "repos/#{workload.repository}/issues/#{workload.ticket_number}"

  defp add_label(workload, label),
    do: write("POST", issue(workload) <> "/labels", ["labels[]=#{label}"])

  defp remove_label(workload, label),
    do: write("DELETE", issue(workload) <> "/labels/" <> URI.encode_www_form(label), [])

  defp comment(workload),
    do: write("POST", issue(workload) <> "/comments", ["body=" <> body(workload)])

  # Every write is tried, and the first that failed is what is returned.
  defp first_error(results) do
    case Enum.find(results, &match?({:error, _}, &1)) do
      nil -> :ok
      error -> error
    end
  end

  defp write(method, endpoint, fields) do
    case GitHub.write(method, endpoint, fields) do
      {:ok, _} ->
        :ok

      {:error, message} ->
        Logger.warning("could not #{method} #{endpoint}: #{message}")
        {:error, message}
    end
  end

  defp body(workload) do
    record = workload.record || %{}

    [
      "**Hephaestium:** #{headline(workload.state, revision?(workload))}",
      reason(workload),
      question(workload.state, record),
      pull_request(record),
      cost(record)
    ]
    |> Enum.reject(&is_nil/1)
    |> Enum.join("\n\n")
  end

  defp headline("completed", true),
    do: "a revision completed and pushed to the pull request's branch."

  defp headline("completed", _), do: "the workload completed and opened a pull request."
  defp headline("failed", _), do: "the workload failed."
  defp headline("timed_out", _), do: "the workload timed out."
  defp headline("blocked", _), do: "the workload is blocked."
  defp headline("cancelled", _), do: "the workload was cancelled."
  defp headline(other, _), do: "the workload is #{other}."

  defp revision?(%Workload{workload: %{"revision" => _}}), do: true
  defp revision?(_), do: false

  defp reason(%{reason: reason}) when reason not in [nil, ""], do: "Why: #{reason}"
  defp reason(_), do: nil

  defp question("blocked", record) do
    case record["question"] do
      question when question not in [nil, ""] ->
        "A question for you: #{question}\n\nAnswer it here and apply `#{@ready}` again to start a new workload."

      _ ->
        "Apply `#{@ready}` again to start a new workload."
    end
  end

  defp question(_, _), do: nil

  defp pull_request(%{"pullRequest" => %{"url" => url}}) when is_binary(url),
    do: "Pull request: #{url}"

  defp pull_request(_), do: nil

  defp cost(record) do
    parts =
      for total <- record["costs"] || [], part = cost_part(total), part != nil, do: part

    if parts == [], do: nil, else: "Cost, as the runtime reported it: #{Enum.join(parts, ", ")}"
  end

  defp cost_part(%{"unit" => "COST_UNIT_USD", "amount" => amount}) when is_number(amount),
    do: "$" <> :erlang.float_to_binary(amount * 1.0, decimals: 3)

  defp cost_part(%{"unit" => "COST_UNIT_TOKENS", "amount" => amount}) when is_number(amount),
    do: "#{trunc(amount)} tokens"

  defp cost_part(%{"unit" => "COST_UNIT_CREDITS", "amount" => amount}) when is_number(amount),
    do: "#{amount} credits"

  defp cost_part(_), do: nil
end
