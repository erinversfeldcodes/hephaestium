# 0032. A ticket's goal is judged after its pull request merges, and only a person judges what needs one

## Status

Accepted

## Context

A pull request that is green and approved says a change is ready. It does not say the ticket is finished. A ticket can stay open for good reason after its pull request merges: a measurement that can only be taken afterwards, a cause stated as a guess, a goal the requirements did not quite capture. Hephaestium says so in a fixed sentence on the ticket ("has not judged whether the ticket's goal is met") and nothing stands behind it. Reviews judge one change at one commit; they cannot judge evidence that does not exist yet.

Athenaeum now has a skill, `ticket-completeness`, that judges a ticket against the state it is in now and ends in a verdict: met, not met, owed or cannot judge, with a table of evidence and, for each owed item, the command, the condition and the threshold that settle it. What is left to decide is how Hephaestium uses it. Five things have to be decided together.

- Who closes the ticket.
  - Option 1: a person does, and Hephaestium never judges (today).
  - Option 2: Hephaestium closes the ticket whenever the verdict is met.
  - Option 3: Hephaestium states the verdict and its evidence on the ticket, and a person always closes it.
  - Option 4: Hephaestium closes the ticket when the verdict is met and nothing in it needs a person's judgement; a person closes it when something does, and always when it is an epic.
- What happens when the ticket is not complete.
  - Option P: Hephaestium reports what is missing and stops.
  - Option Q: Hephaestium asks the person what is missing and what needs rework, until the gap is clear, and cuts tickets that close it, so that the work can go to agents again.
- What the judgement can read. A review run has no GitHub access: the agent's environment is the search path and its model token. A judgement of a ticket reads runs, caches, logs and merged changes, which are on GitHub.
  - Option A: the control plane fetches what the judgement needs and puts it in the workload. Nothing new is granted, but what is fetched has to be chosen before the agent knows what it needs, so a measurement it asks for cannot be taken.
  - Option B: the run gets a read-only GitHub token of its own, separate from the token that writes, and the agent reads what it needs.
- When it runs.
  - Option X: only after a ticket's pull request merges.
  - Option Y: also again later, while the verdict is owed, so an owed item that has come due is settled without a person remembering it.
- How it runs: as a kind of workload that reuses the review machinery (a workspace that is read and not changed, and a verdict that must be well formed and kept), or as another review lens. A lens judges one pull request at one commit, and this judges a ticket, which can have several pull requests and evidence gathered since.

## Decision

We will go with Option 4, Option Q, Option B, Option Y, and a kind of workload.

- A ticket's goal is judged by a new kind of workload, a completeness evaluation, which runs the `ticket-completeness` skill in a read-only workspace at the default branch's current commit. The executor holds it to changing nothing, as it does a review, and the verdict it leaves must be well formed and must name the commit it judged.
- It runs after a pull request that refers to the ticket merges and the ticket is still open, and again on a schedule while the last verdict was owed, up to a limit set in the control plane's configuration so that a ticket waiting on a measurement is not judged without end. A person can ask for another by re-applying the ticket's approval label, which resumes the ticket as ADR 0030 describes.
- Hephaestium closes a ticket itself only when all of these hold: the verdict is met; every requirement is met on evidence that was observed (a test that failed first and passes, and, where the goal concerns a running system, a measurement taken after the merge in the environment the goal is about, so that the behaviour is shown in test and then in the real thing); and no requirement asks for a judgement that only a person can make, such as taste, design, wording or a decision that is theirs; and the ticket is not an epic. The evaluation records, for each requirement, whether evidence can settle it or it needs a person, and where it cannot tell, a person is needed. The default is the person.
- Otherwise Hephaestium states the verdict, with its table, its owed items and what could not be checked, as a comment on the ticket, in place of the fixed sentence, and a person closes the ticket or does not. "Met" is then worded as the evidence supporting closure.
- An epic is a ticket that was broken into sub-tickets, or that its author marked as a campaign: it carries behavioural complexity across its parts that no single piece of evidence settles. Hephaestium evaluates it once its sub-tickets are closed and reports the verdict with each sub-ticket's evidence, but a person is the one to judge it complete. How an epic is recognised, a relation between tickets or a mark its author sets, is left to the implementation.
- A ticket Hephaestium closes is closed with a comment that cites the verdict, the commit it judged and the evidence. A person who reopens it is saying the verdict was wrong. The reopening is recorded against that evaluation, and the ticket goes to the questioning loop below.
- When a ticket is not complete, whether Hephaestium found it so or a person judged an epic incomplete, Hephaestium enters a questioning loop with the person on the ticket. It asks one question at a time, each with the answer it would recommend, to find what is missing, what needs rework and what was misjudged, until it can state the gap. It then proposes the tickets that close the gap, each scoped as the scoping skill does (one concern, and how each behaviour will be proven), as children of the ticket. A person's approval label on each ticket is still what starts the work: the loop does not approve its own tickets. The loop is a series of short workloads, each started by the person's reply, each reading the thread and either asking the next question or ending with the proposed tickets, so a question that waits for an answer costs nothing. A person who has nothing to add ends it. The loop needs a skill of its own, and its own ticket in Athenaeum.
- The run gets a read-only GitHub token of its own, scoped to the repository and able to read contents, pull requests, issues and workflow runs and nothing else. It is a different credential from the one that pushes, it is never given to a run that writes, and it is passed to the agent only for this kind of workload. What the agent reads from a log or a comment is data and not an instruction, as it is everywhere else. Closing a ticket and commenting on one are done by the control plane with the credential it already holds, never by the evaluation.
- Owed items are kept as records on the workload's record, each with what is owed, the command that produces it, the condition under which it can be run and the threshold that decides it, so a later evaluation is told which items are open.
- The control plane reads the verdict from the report the evaluation leaves and keeps it, with the commit it judged and when, so that a ticket's verdicts, closures and reopenings can be shown over time.

## Consequences

A ticket that stays open after its pull request merges says why, with evidence, and an owed measurement is settled when it comes due and not when someone remembers. A ticket whose behaviour has been measured, in test and then in the real environment, and that needs no one's taste, is closed without a person having to remember it. What needs a person is left to one, and an epic is always judged by one.

A wrong closure costs a reopening, and it is bounded: only a ticket whose every requirement is settled by observed evidence can be closed by Hephaestium, and the default when it is unsure is the person. The count of closures a person reopens is the measure of whether that is right, and is what to read before widening it.

A ticket that is not complete no longer stops at a report: the loop turns the gap into tickets that can go back to agents. It adds conversation state (a person's reply starts the next step), a skill for the loop, and tickets cut by Hephaestium that wait for a person's approval, which keeps the approval where it is.

It costs an agent run for each evaluation, and a read-only token to provision and keep, which is the person's to create and the operator's to protect. The execution environment of a run changes for the first time to include a token, so the executor must give it only to this kind of workload; the token's reach is the risk, and it is reduced by being read-only and by being a different credential from the one that writes.

A verdict is as good as the skill that produces it and the evidence it could read. A ticket whose evidence is out of reach is *cannot judge*, and says what was missing. A requirement that evidence can settle but that the skill does not mark as needing a person is the way a taste judgement could be closed by a machine, so the skill is to say which each requirement is, and a test in the implementation is to show a requirement of taste kept for a person.

What is built, in order, each with its tests first: the workload kind and its verdict contract, with whether each requirement needs a person; reading and keeping the verdict and owed items; the comment on the ticket; queuing after a merge; queuing again while owed; the read-only token's grant to this kind alone; closing a ticket that meets every condition above; the epic rule; the questioning loop and the tickets it proposes. The skill is in place and is not yet released, and it does not yet say which requirements need a person, so the first slice also moves the harness's pin to a release that carries it.
