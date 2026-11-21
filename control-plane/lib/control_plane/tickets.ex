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

  alias ControlPlane.{
    Account,
    CompletenessComment,
    Evaluations,
    Gaps,
    GitHub,
    PullRequestComments,
    Workload
  }

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
  def started(%Workload{workload: %{"review" => _}}), do: :ok
  def started(%Workload{workload: %{"completeness" => _}}), do: :ok
  def started(%Workload{workload: %{"gap" => _}}), do: :ok

  def started(workload) do
    first_error([
      add_label(workload, @running),
      remove_label(workload, @ready)
    ])
  end

  @doc "The workload has ended: leave the label that says how, and a comment."
  @spec ended(%Workload{}) :: :ok | {:error, String.t()}
  # A review reads a pull request and says nothing on the ticket by itself: what its
  # verdict means for the ticket is decided where the verdicts are counted. What it found
  # is written on the pull request, where a person reviewing it looks.
  def ended(%Workload{workload: %{"review" => _}} = workload),
    do: PullRequestComments.review(workload)

  # A completeness evaluation judges a ticket and changes no label: what it found is a
  # comment on the ticket, and a person or a later decision acts on it.
  def ended(%Workload{workload: %{"completeness" => _}} = workload), do: evaluated(workload)

  # A step of the questioning says what it found as a comment, and changes no label: a
  # person's reply, and a person's approval of any ticket it proposed, are what act.
  def ended(%Workload{workload: %{"gap" => _}} = workload), do: stepped(workload)

  # A revision's account is on the pull request as well as the ticket.
  def ended(%Workload{} = workload),
    do: first_error([end_ticket(workload), PullRequestComments.revision(workload)])

  defp evaluated(%Workload{state: "completed", record: %{"completeness" => verdict}} = workload)
       when is_map(verdict) do
    closing =
      Evaluations.closing?(workload.repository) and not Evaluations.epic?(workload) and
        Evaluations.closes?(verdict)

    # The reason is on the ticket before it is closed, so a closure is never without one.
    with :ok <- post(workload, CompletenessComment.render(verdict, closing)),
         :ok <- if(closing, do: close(workload), else: :ok) do
      # What the verdict found missing is asked about, when the goal is not met.
      Gaps.after_verdict(workload, verdict)
      :ok
    end
  end

  defp evaluated(%Workload{state: "completed"} = workload),
    do:
      post(
        workload,
        "**Hephaestium:** a completeness evaluation ended but left no verdict in its record."
      )

  # A verdict that was refused is tried once more before anyone is told: the first failure is
  # the agent's to correct, and a comment for each would be noise. If it cannot be tried, or
  # the retry is refused too, the ticket is told.
  defp evaluated(%Workload{} = workload) do
    with true <- Evaluations.retryable?(workload),
         {:started, _id} <- Evaluations.retry(workload) do
      :ok
    else
      _ -> announce_failed(workload)
    end
  end

  defp close(workload),
    do: write("PATCH", issue(workload), ["state=closed", "state_reason=completed"])

  # A step's own words are the comment, as written, up to what a comment can hold.
  @comment_limit 60_000

  defp stepped(
         %Workload{state: "completed", record: %{"gap" => %{"step" => "proposal"} = kept}} =
           workload
       ) do
    {made, problems} = Gaps.create(kept["tickets"] || [], workload.repository)
    post(workload, CompletenessComment.proposed(made, problems, kept["tickets"] || []))
  end

  defp stepped(%Workload{state: "completed", record: %{"gap" => %{"text" => text}}} = workload)
       when is_binary(text),
       do:
         post(
           workload,
           "**Hephaestium:** a step in working out what is missing.\n\n" <>
             String.slice(text, 0, @comment_limit)
         )

  defp stepped(%Workload{state: "completed"} = workload),
    do:
      post(
        workload,
        "**Hephaestium:** a step in working out what is missing ended but left nothing in its record."
      )

  # A step the executor refused is tried once more before anyone is told, as a refused
  # verdict is: the first failure is the agent's to correct. If it cannot be tried, or the
  # retry is refused too, the ticket is told.
  defp stepped(%Workload{} = workload) do
    with true <- Gaps.retryable?(workload),
         {:started, _id} <- Gaps.retry(workload) do
      :ok
    else
      _ -> announce_step_failed(workload)
    end
  end

  defp announce_step_failed(%Workload{} = workload) do
    why = if workload.reason in [nil, ""], do: workload.state, else: workload.reason

    post(
      workload,
      "**Hephaestium:** a step in working out what is missing could not be completed: #{why}. A person's next reply starts another."
    )
  end

  defp announce_failed(%Workload{} = workload) do
    why = if workload.reason in [nil, ""], do: workload.state, else: workload.reason
    second_attempt = workload.workload["completeness"]["refused"] not in [nil, ""]

    post(workload, CompletenessComment.failure(why, second_attempt))
  end

  defp post(workload, text),
    do: write("POST", issue(workload) <> "/comments", ["body=" <> text])

  defp end_ticket(%Workload{state: state} = workload) when state in ["failed", "timed_out"] do
    first_error([
      add_label(workload, @failed),
      remove_label(workload, @running),
      comment(workload)
    ])
  end

  defp end_ticket(%Workload{state: "blocked"} = workload) do
    first_error([
      add_label(workload, @blocked),
      remove_label(workload, @running),
      comment(workload)
    ])
  end

  defp end_ticket(%Workload{state: "cancelled"} = workload) do
    first_error([remove_label(workload, @running), comment(workload)])
  end

  defp end_ticket(%Workload{state: "completed"} = workload), do: first_error([comment(workload)])

  defp end_ticket(_workload), do: :ok

  @done "heph:done"

  @doc """
  The workload's pull request has been looked at and the ticket settled: `:done` when
  its required checks are green (or it was merged), `{:failed, names}` when some
  are not, `:abandoned` when it was closed without merging.
  """
  @spec settled(%Workload{}, :done | {:failed, [String.t()]} | :abandoned) ::
          :ok | {:error, String.t()}
  def settled(workload, verdict, notes \\ nil)

  def settled(workload, :done, notes), do: settled(workload, {:done, nil}, notes)

  def settled(workload, {:done, merge}, notes) do
    first_error([
      add_label(workload, @done),
      remove_label(workload, @running),
      note(
        workload,
        ready_text(workload, notes) <> merged(merge) <> if(notes, do: "\n\n" <> notes, else: "")
      )
    ])
  end

  # The checks were green and the reviews did not approve it.
  def settled(workload, {:review_failed, lenses, extra}, _notes) do
    first_error([
      add_label(workload, @failed),
      remove_label(workload, @running),
      note(
        workload,
        "the pull request's checks are green and its review did not approve it: #{Enum.join(lenses, ", ")}.\n\n#{extra}"
      )
    ])
  end

  def settled(workload, {:failed, names}, notes),
    do: settled(workload, {:failed, names, nil}, notes)

  def settled(workload, {:failed, names, extra}, _notes) do
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

  def settled(workload, :abandoned, _notes) do
    first_error([
      remove_label(workload, @running),
      note(workload, "the pull request was closed without being merged.")
    ])
  end

  @resume_removes ["heph:ready", "heph:failed", "heph:done", "heph:blocked"]

  @doc """
  A maintainer re-approved a ticket whose pull request is open, and it is being worked on
  again (#131): the running label is added, whichever state labels the ticket has are
  removed (and no label it does not have is asked to go), and a comment says who resumed
  which pull request.
  """
  @spec resumed(%Workload{}, [String.t()], String.t(), String.t() | nil) ::
          :ok | {:error, String.t()}
  def resumed(%Workload{} = workload, labels, login, url) do
    first_error(
      [add_label(workload, @running)] ++
        for(label <- @resume_removes, label in labels, do: remove_label(workload, label)) ++
        [
          write("POST", issue(workload) <> "/comments", [
            "body=**Hephaestium:** resumed by #{login}. The pull request #{url || get_in(workload.record || %{}, ["pullRequest", "url"])} will be looked at again, and revised if its checks or its reviews call for it."
          ])
        ]
    )
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

  @doc """
  The pull request's reviews found something that blocks and a revision has been
  started: the ticket says which lenses, and that the agent will address the
  blocking findings on the same branch. The labels do not change.
  """
  @spec revising_for_review(%Workload{}, [String.t()], pos_integer(), pos_integer()) ::
          :ok | {:error, String.t()}
  def revising_for_review(workload, names, attempt, max) do
    note(
      workload,
      "the pull request's review found problems that block it: #{Enum.join(names, ", ")}. Starting revision #{attempt} of #{max}: the agent will address the blocking findings on the same branch."
    )
  end

  @doc """
  The pull request's checks are green but its branch is behind its base, and a revision
  has been started to bring the base in: the ticket says how many commits came in, and
  that the agent will confirm the change still stands on them and push to the same
  branch. The labels do not change.
  """
  @spec revising_for_base(%Workload{}, non_neg_integer(), pos_integer(), pos_integer()) ::
          :ok | {:error, String.t()}
  def revising_for_base(workload, commits, attempt, max) do
    note(
      workload,
      "the base branch has moved since the pull request was opened (#{commits} #{if commits == 1, do: "commit", else: "commits"} came in). Starting revision #{attempt} of #{max}: the agent will confirm the change still stands on it and push to the same branch."
    )
  end

  # What a green, approved pull request claims: that its checks are green and, if reviews
  # were asked for, that they approved it. Neither says the ticket's goal is met. Where the
  # ticket is judged after the merge, the comment says it will be; elsewhere it says the goal
  # is for a person to judge, and says when no review was asked for.
  defp ready_text(workload, notes) do
    # A review approved it only if the notes carry a verdict line for a lens. The header
    # alone is also written when nothing ran, with only what no lens looked at.
    reviewed? = is_binary(notes) and Regex.match?(~r/^- [a-z0-9][a-z0-9-]*: /m, notes)

    if(
      reviewed?,
      do: "the pull request's checks are green and its reviews approved it.",
      else: "the pull request's checks are green. No review was asked for."
    ) <> goal_sentence(Evaluations.enabled?(workload.repository))
  end

  defp goal_sentence(true),
    do:
      " Hephaestium judges whether the ticket's goal is met once it merges, against what is on the default branch, and comments with its verdict."

  defp goal_sentence(false),
    do:
      " Hephaestium has not judged whether the ticket's goal is met: that is for a person, against the ticket."

  defp merged(nil), do: ""
  defp merged(:merged), do: " Hephaestium merged it."
  defp merged({:not_merged, reason}), do: " Not merged: #{reason}."

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
      account(workload),
      pull_request(record),
      cost(record)
    ]
    |> Enum.reject(&is_nil/1)
    |> Enum.join("\n\n")
  end

  defp headline("completed", true),
    do: "a revision completed and pushed to the pull request's branch."

  defp headline("completed", _), do: "the workload completed and opened a pull request."
  defp headline("failed", true), do: "the revision failed."
  defp headline("failed", _), do: "the workload failed."
  defp headline("timed_out", true), do: "the revision timed out."
  defp headline("timed_out", _), do: "the workload timed out."
  defp headline("blocked", _), do: "the workload is blocked."
  defp headline("cancelled", _), do: "the workload was cancelled."
  defp headline(other, _), do: "the workload is #{other}."

  defp revision?(%Workload{workload: %{"revision" => _}}), do: true
  defp revision?(_), do: false

  # A revision says what its agent found and did, in the agent's words: a person reading
  # the ticket should not have to look in the run's events. A revision that stopped to
  # ask has its question instead.
  defp account(%Workload{state: state} = workload) when state != "blocked" do
    if revision?(workload), do: Account.section(workload.id)
  end

  defp account(_workload), do: nil

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
