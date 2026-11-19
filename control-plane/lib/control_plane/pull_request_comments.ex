defmodule ControlPlane.PullRequestComments do
  @moduledoc """
  What a review found, and what a revision's agent answered, on the pull request where
  a person reviewing it looks (#127). Until this, both were only on the ticket and in the
  run's events.

  These are comments, and only comments. Nothing is submitted as a GitHub review, an
  approval or a request for changes: the repository's approval rule is for a person, and
  an agent's verdict does not count toward it.

  What an agent wrote is data to show and never an instruction: it goes in a fenced block
  its own text cannot end, cleaned and cut (see `ControlPlane.Account`). Every other word
  in a comment is ours, and what the contract fixes (a verdict, a severity, a commit) is
  checked before it is repeated.

  A comment is written once: the writing is claimed on the workload first, and the claim
  is given back if the write fails, so it is tried again and never written twice. A
  failure to write is returned, and never changes how a workload ended.
  """

  require Logger

  alias ControlPlane.{Account, Finding, GitHub, Workload, Workloads}

  @verdicts ["approved", "needs_revision", "failed"]

  @doc "Write what a review found on its pull request, once."
  @spec review(%Workload{}) :: :ok | {:error, String.t()}
  def review(%Workload{workload: %{"review" => review}} = workload) do
    post(workload, review["pullRequestNumber"], fn -> review_body(workload, review) end)
  end

  def review(_workload), do: :ok

  @doc """
  Write a revision's account on its pull request, once. A revision that stopped to ask a
  question, or was cancelled, has no account to give, and a workload that is not a
  revision has nothing here.
  """
  @spec revision(%Workload{}) :: :ok | {:error, String.t()}
  def revision(%Workload{state: state, workload: %{"revision" => revision}} = workload)
      when state in ["completed", "failed", "timed_out"] do
    post(workload, revision["pullRequestNumber"], fn -> revision_body(workload, revision) end)
  end

  def revision(_workload), do: :ok

  defp post(workload, number, body) do
    with {:ok, number} <- pull_request_number(number),
         :ok <- claim(workload) do
      case GitHub.write("POST", "repos/#{workload.repository}/issues/#{number}/comments", [
             "body=" <> body.()
           ]) do
        {:ok, _} ->
          :ok

        {:error, message} ->
          Workloads.release_report(workload.id)
          Logger.warning("could not comment on #{workload.repository}##{number}: #{message}")
          {:error, message}
      end
    else
      :already -> :ok
      {:error, _} = error -> error
    end
  end

  defp claim(workload) do
    case Workloads.claim_report(workload.id) do
      :ok -> :ok
      :already -> :already
    end
  end

  defp pull_request_number(number) when is_binary(number) do
    if Regex.match?(~r/\A[1-9][0-9]{0,8}\z/, number),
      do: {:ok, number},
      else: {:error, "the pull request number #{inspect(number)} is not one"}
  end

  defp pull_request_number(number),
    do: {:error, "the pull request number #{inspect(number)} is not one"}

  # A review.

  defp review_body(
         %Workload{state: "completed", record: %{"review" => %{} = verdict}} = w,
         review
       ) do
    findings = for %{} = finding <- List.wrap(verdict["findings"]), do: finding
    blocking = Enum.count(findings, &(&1["blocking"] == true))

    [
      heading(review, verdict["commit"] || commit(w), "**#{said(verdict["verdict"])}**"),
      summary(findings, blocking),
      Enum.map(findings, &finding_block/1),
      notes_block(verdict["notes"]),
      standing()
    ]
    |> List.flatten()
    |> Enum.reject(&is_nil/1)
    |> Enum.join("\n\n")
  end

  defp review_body(%Workload{state: "completed"} = w, review) do
    [
      heading(review, commit(w), "**left no verdict** that names this commit and this lens"),
      "That is no approval.",
      standing()
    ]
    |> Enum.join("\n\n")
  end

  defp review_body(%Workload{state: state, reason: reason} = w, review) do
    [
      heading(review, commit(w), "**could not review** (#{state})"),
      if(reason in [nil, ""], do: nil, else: Account.block(reason)),
      "That is no approval.",
      standing()
    ]
    |> Enum.reject(&is_nil/1)
    |> Enum.join("\n\n")
  end

  defp heading(review, sha, what) do
    "**Hephaestium review, `#{lens(review["lens"])}`**, of `#{short(sha)}`: " <> what
  end

  defp lens(name) when is_binary(name) do
    if Regex.match?(~r/\A[a-z0-9][a-z0-9-]{0,40}\z/, name), do: name, else: "a lens"
  end

  defp lens(_), do: "a lens"

  defp commit(%Workload{workload: %{"repository" => %{"baseCommit" => sha}}}), do: sha
  defp commit(_), do: ""

  defp short(sha) when is_binary(sha) do
    if Regex.match?(~r/\A[0-9a-f]{40}\z/, sha), do: String.slice(sha, 0, 7), else: "a commit"
  end

  defp short(_), do: "a commit"

  defp said(verdict) when verdict in @verdicts, do: verdict
  defp said(_), do: "an unrecognised verdict"

  defp summary([], _blocking), do: "No findings."

  defp summary(findings, blocking) do
    "#{count(length(findings), "finding")}, #{blocking} of them blocking."
  end

  defp count(1, word), do: "1 #{word}"
  defp count(n, word), do: "#{n} #{word}s"

  # One finding. What is ours is the line above the block; what the agent wrote is in it.
  defp finding_block(finding) do
    severity =
      if Regex.match?(~r/\AP[0-3]\z/, to_string(finding["severity"])),
        do: finding["severity"],
        else: "?"

    blocking = if finding["blocking"] == true, do: "blocking", else: "not blocking"

    text =
      [
        text(finding["title"]),
        Finding.location_line(finding["location"]),
        labelled("Evidence", finding["evidence"]),
        labelled("What to revise", finding["revision"])
      ]
      |> Enum.reject(&is_nil/1)
      |> Enum.join("\n")

    "**[#{severity}] #{blocking}**\n\n" <> Account.block(text)
  end

  defp notes_block(notes) when is_list(notes) do
    case notes |> Enum.filter(&is_binary/1) |> Enum.map(&("- " <> Account.clean(&1))) do
      [] -> nil
      lines -> "Notes:\n\n" <> Account.block(Enum.join(lines, "\n"))
    end
  end

  defp notes_block(_), do: nil

  defp text(value) when is_binary(value), do: Account.clean(value)
  defp text(_), do: nil

  defp labelled(label, value) when is_binary(value), do: "#{label}: " <> Account.clean(value)
  defp labelled(_label, _value), do: nil

  defp standing do
    "This is a comment from Hephaestium. It is not a GitHub review, and the repository's approval is for a person."
  end

  # A revision.

  defp revision_body(%Workload{} = workload, revision) do
    attempt = attempt(revision["attempt"])

    [
      "**Hephaestium, revision #{attempt}:** #{ended_as(workload)}",
      Account.section(workload.id),
      standing()
    ]
    |> Enum.join("\n\n")
  end

  defp ended_as(%Workload{state: "completed"}), do: "it pushed to this branch."
  defp ended_as(%Workload{state: "failed"}), do: "it ended without a change to push."
  defp ended_as(%Workload{state: "timed_out"}), do: "it timed out."

  defp attempt(n) when is_integer(n) and n > 0 and n < 100, do: n
  defp attempt(_), do: "?"
end
