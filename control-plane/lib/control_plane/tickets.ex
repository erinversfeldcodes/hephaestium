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
    Amounts,
    CompletenessComment,
    Dashboard,
    Evaluations,
    Gaps,
    Tracker,
    PullRequestComments,
    RevisionNeeded,
    Spend,
    Workload,
    Workloads
  }

  @ready "heph:ready"
  @running "heph:running"
  @failed "heph:failed"
  @blocked "heph:blocked"

  # The units a cost is in, in the order a person reads them: dollars, tokens, credits.
  @units ["COST_UNIT_USD", "COST_UNIT_TOKENS", "COST_UNIT_CREDITS"]

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

  @doc """
  The workload has ended: leave the label that says how, and a comment, and have its pull
  request settled at once when webhooks are on, because no webhook says that it ended.
  """
  @spec ended(%Workload{}) :: :ok | {:error, String.t()}
  def ended(%Workload{} = workload) do
    result = finish(workload)
    ControlPlane.Webhooks.wake_settling(workload)
    result
  end

  # A review reads a pull request and says nothing on the ticket by itself: what its
  # verdict means for the ticket is decided where the verdicts are counted. What it found
  # is written on the pull request, where a person reviewing it looks.
  defp finish(%Workload{workload: %{"review" => _}} = workload),
    do: PullRequestComments.review(workload)

  # A completeness evaluation judges a ticket and changes no label: what it found is a
  # comment on the ticket, and a person or a later decision acts on it.
  defp finish(%Workload{workload: %{"completeness" => _}} = workload), do: evaluated(workload)

  # A step of the questioning says what it found as a comment, and changes no label: a
  # person's reply, and a person's approval of any ticket it proposed, are what act.
  defp finish(%Workload{workload: %{"gap" => _}} = workload), do: stepped(workload)

  # A revision that failed may only have found its pull request merged and its branch gone.
  # Then the work did not fail: the workload is not needed, and the ticket keeps the label the
  # merge gave it and gets no comment.
  defp finish(%Workload{state: state, workload: %{"revision" => _}} = workload)
       when state in ["failed", "timed_out"] do
    case RevisionNeeded.check(workload) do
      {:not_needed, reason} -> Workloads.not_needed(workload.id, reason)
      :needed -> revised(workload)
    end
  end

  defp finish(%Workload{} = workload), do: revised(workload)

  # A revision's account is on the pull request as well as the ticket.
  defp revised(%Workload{} = workload),
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

  defp close(workload), do: ok(Tracker.close_ticket(workload.repository, workload.ticket_number))

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
    do: comment_on(workload, text)

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
          comment_on(
            workload,
            "**Hephaestium:** resumed by #{login}. The pull request #{url || get_in(workload.record || %{}, ["pullRequest", "url"])} will be looked at again, and revised if its checks or its reviews call for it."
          )
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

    comment_on(workload, "**Hephaestium:** #{text}\n\nPull request: #{url}")
  end

  defp add_label(workload, label),
    do: ok(Tracker.add_label(workload.repository, workload.ticket_number, label))

  defp remove_label(workload, label),
    do: ok(Tracker.remove_label(workload.repository, workload.ticket_number, label))

  defp comment(workload), do: comment_on(workload, body(workload))

  defp comment_on(workload, text),
    do: ok(Tracker.comment_on_ticket(workload.repository, workload.ticket_number, text))

  # Every write is tried, and the first that failed is what is returned.
  defp first_error(results) do
    case Enum.find(results, &match?({:error, _}, &1)) do
      nil -> :ok
      error -> error
    end
  end

  defp ok(:ok), do: :ok
  defp ok({:error, message}), do: {:error, message}

  defp body(workload) do
    record = workload.record || %{}

    [
      "**Hephaestium:** #{headline(workload.state, revision?(workload), record)}",
      reason(workload),
      question(workload.state, workload, record),
      account(workload),
      pull_request(record),
      cost_section(record),
      spend_section(workload)
    ]
    |> Enum.reject(&is_nil/1)
    |> Enum.join("\n\n")
  end

  # A revision that changed only the pull request's description pushed no commit, and says
  # so: the branch, and the approvals and checks on it, are as they were.
  defp headline("completed", true, %{"pullRequest" => %{"nothingPushed" => true}}),
    do: "a revision completed and updated the pull request's description; nothing was pushed."

  defp headline("completed", true, _),
    do: "a revision completed and pushed to the pull request's branch."

  defp headline("completed", _, _), do: "the workload completed and opened a pull request."
  defp headline("failed", true, _), do: "the revision failed."
  defp headline("failed", _, _), do: "the workload failed."
  defp headline("timed_out", true, _), do: "the revision timed out."
  defp headline("timed_out", _, _), do: "the workload timed out."
  defp headline("blocked", _, _), do: "the workload is blocked."
  defp headline("cancelled", _, _), do: "the workload was cancelled."
  defp headline(other, _, _), do: "the workload is #{other}."

  defp revision?(%Workload{workload: %{"revision" => _}}), do: true
  defp revision?(_), do: false

  # A revision says what its agent found and did, in the agent's words: a person reading
  # the ticket should not have to look in the run's events. So does a first run that ended
  # without a pull request (it failed, timed out or was cancelled), because what its agent
  # learned, that it could not reproduce the bug and what to look at next, is what the person
  # who approved the ticket needs to act on. A first run that completed has its account on its
  # pull request. A run that stopped to ask has its question instead, quoted as data as the
  # account is, because the agent wrote it.
  #
  # What an agent wrote is public once it is on a ticket, so when the dashboard's address is
  # configured the ticket points to the run there instead, and the words are read behind the
  # login. With no address there is nothing to point to and the words stay on the ticket.
  defp account(%Workload{state: state} = workload) when state != "blocked" do
    if revision?(workload) or state != "completed" do
      Account.public_section(workload)
    end
  end

  defp account(_workload), do: nil

  defp reason(%{reason: reason}) when reason not in [nil, ""], do: "Why: #{reason}"
  defp reason(_), do: nil

  defp question("blocked", workload, record) do
    case record["question"] do
      question when question not in [nil, ""] ->
        case Dashboard.run_link(workload) do
          nil ->
            "A question for you:\n\n#{Account.block(question)}\n\nAnswer it in the form on the dashboard, under the question, signed in: an answer written as a comment on this ticket is not read by the agent. Applying `#{@ready}` again without answering asks it again."

          link ->
            "The agent has a question for you. Read it, signed in, on the dashboard: #{link}\n\nAnswer it there, in the form under the question: an answer written as a comment on this ticket is not read by the agent."
        end

      _ ->
        "Apply `#{@ready}` again to start a new workload."
    end
  end

  defp question(_, _, _), do: nil

  defp pull_request(%{"pullRequest" => %{"url" => url}}) when is_binary(url),
    do: "Pull request: #{url}"

  defp pull_request(_), do: nil

  @doc """
  What the ticket has spent, as the status comment says it: over all its workloads and by kind
  of step, and for a ticket under an epic, what the epic has spent over its tickets. It reads
  the ledger, which has the run that just ended, since a record is kept before the ticket is
  told. Units are never added to each other. Nothing when nothing was spent.
  """
  @spec spend_section(%Workload{}) :: String.t() | nil
  def spend_section(%Workload{repository: repository, ticket_number: number} = workload) do
    ticket = Spend.ticket(repository, number)

    if ticket.totals == %{} do
      nil
    else
      [
        "Spent on this ticket so far: #{Amounts.describe(ticket.totals)} across #{count(ticket.workloads, "workload")}.",
        kinds(ticket.by_kind),
        epic_spend(repository, workload)
      ]
      |> Enum.reject(&is_nil/1)
      |> Enum.join("\n")
    end
  end

  defp kinds(by_kind) do
    parts =
      for {kind, label} <- Spend.kinds(),
          totals = by_kind[kind],
          totals not in [nil, %{}],
          do: "#{label} #{Amounts.describe(totals, " and ")}"

    if parts == [], do: nil, else: "By kind of step: " <> Enum.join(parts, ", ")
  end

  defp epic_spend(repository, %Workload{workload: %{"ticket" => %{"parent" => parent}}}) do
    with {epic, ""} when epic > 0 <- Integer.parse(to_string(parent)),
         %{totals: totals} = rollup when totals != %{} <- Spend.epic(repository, epic) do
      "Its epic ##{epic} has spent #{Amounts.describe(totals)} across #{count(length(rollup.tickets), "ticket")} so far."
    else
      _ -> nil
    end
  end

  defp epic_spend(_repository, _workload), do: nil

  defp count(1, noun), do: "1 #{noun}"
  defp count(number, noun), do: "#{number} #{noun}s"

  @doc """
  What a run spent, as the status comment says it: the total, and for a run that read its own
  change under lenses, what each kind of agent it started spent, so that what went on reading
  can be told apart from what went on the work. Units are never added to each other.
  """
  @spec cost_section(map()) :: String.t() | nil
  def cost_section(record) do
    case {cost(record), breakdown(record["steps"])} do
      {nil, _} -> nil
      {total, nil} -> total
      {total, parts} -> total <> "\n" <> parts
    end
  end

  @step_kinds [
    {"KIND_IMPLEMENTATION", "the implementation"},
    {"KIND_REVIEW", "reading under lenses"},
    {"KIND_REVISION", "fixing what they found"}
  ]

  defp breakdown(steps) when is_list(steps) do
    parts =
      for {kind, label} <- @step_kinds,
          spent = spent_by(steps, kind),
          spent != [] do
        "#{label} #{Enum.join(spent, " and ")}"
      end

    if parts == [], do: nil, else: "Of that: " <> Enum.join(parts, ", ")
  end

  defp breakdown(_), do: nil

  # What the steps of one kind spent, in each unit on its own.
  defp spent_by(steps, kind) do
    steps
    |> Enum.filter(&(&1["kind"] == kind))
    |> Enum.flat_map(&(&1["costs"] || []))
    |> Enum.group_by(& &1["unit"], & &1["amount"])
    |> Enum.sort_by(fn {unit, _} -> Enum.find_index(@units, &(&1 == unit)) || length(@units) end)
    |> Enum.map(fn {unit, amounts} ->
      cost_part(%{"unit" => unit, "amount" => amounts |> Enum.filter(&is_number/1) |> Enum.sum()})
    end)
    |> Enum.reject(&is_nil/1)
  end

  defp cost(record) do
    parts =
      for total <- record["costs"] || [], part = cost_part(total), part != nil, do: part

    if parts == [], do: nil, else: "Cost, as the runtime reported it: #{Enum.join(parts, ", ")}"
  end

  defp cost_part(%{"unit" => "COST_UNIT_" <> unit, "amount" => amount}) when is_number(amount),
    do: Amounts.format(String.downcase(unit), amount)

  defp cost_part(_), do: nil
end
