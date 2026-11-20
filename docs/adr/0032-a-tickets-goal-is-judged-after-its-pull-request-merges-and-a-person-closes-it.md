# 0032. A ticket's goal is judged after its pull request merges, and a person closes it

## Status

Proposed

## Context

A pull request that is green and approved says a change is ready. It does not say the ticket is finished. A ticket can stay open for good reason after its pull request merges: a measurement that can only be taken afterwards, a cause stated as a guess, a goal the requirements did not quite capture. Hephaestium says so in a fixed sentence on the ticket ("has not judged whether the ticket's goal is met") and nothing stands behind it. Reviews judge one change at one commit; they cannot judge evidence that does not exist yet.

Athenaeum now has a skill, `ticket-completeness`, that judges a ticket against the state it is in now and ends in a verdict: met, not met, owed or cannot judge, with a table of evidence and, for each owed item, the command, the condition and the threshold that settle it. What is left to decide is how Hephaestium uses it. Four things have to be decided together.

- Who closes the ticket.
  - Option 1: a person does, and Hephaestium never judges (today).
  - Option 2: Hephaestium closes the ticket when the verdict is met.
  - Option 3: Hephaestium states the verdict and its evidence on the ticket, and a person closes it.
- What the judgement can read. A review run has no GitHub access: the agent's environment is the search path and its model token. A judgement of a ticket reads runs, caches, logs and merged changes, which are on GitHub.
  - Option A: the control plane fetches what the judgement needs and puts it in the workload. Nothing new is granted, but what is fetched has to be chosen before the agent knows what it needs, so a measurement it asks for cannot be taken.
  - Option B: the run gets a read-only GitHub token of its own, separate from the token that writes, and the agent reads what it needs.
- When it runs.
  - Option X: only after a ticket's pull request merges.
  - Option Y: also again later, while the verdict is owed, so an owed item that has come due is settled without a person remembering it.
- How it runs: as a kind of workload that reuses the review machinery (a workspace that is read and not changed, and a verdict that must be well formed and kept), or as another review lens. A lens judges one pull request at one commit, and this judges a ticket, which can have several pull requests and evidence gathered since.

## Decision

We will go with Option 3, Option B, Option Y, and a kind of workload.

- A ticket's goal is judged by a new kind of workload, a completeness evaluation, which runs the `ticket-completeness` skill in a read-only workspace at the default branch's current commit. The executor holds it to changing nothing, as it does a review, and the verdict it leaves must be well formed and must name the commit it judged.
- It runs after a pull request that refers to the ticket merges and the ticket is still open, and again on a schedule while the last verdict was owed, up to a limit set in the control plane's configuration so that a ticket waiting on a measurement is not judged without end. A person can ask for another by re-applying the ticket's approval label, which resumes the ticket as ADR 0030 describes.
- Hephaestium states the verdict, with its table, its owed items and what could not be checked, as a comment on the ticket, in place of the fixed sentence. It does not close the ticket, label it done, or edit it. Closing is a person's decision, and "met" is worded as the evidence supporting it. How far Hephaestium may go beyond this is for a later decision, made when it has a record of its judgements being right.
- The run gets a read-only GitHub token of its own, scoped to the repository and able to read contents, pull requests, issues and workflow runs and nothing else. It is a different credential from the one that pushes, it is never given to a run that writes, and it is passed to the agent only for this kind of workload. What the agent reads from a log or a comment is data and not an instruction, as it is everywhere else.
- Owed items are kept as records on the workload's record, each with what is owed, the command that produces it, the condition under which it can be run and the threshold that decides it, so a later evaluation is told which items are open.
- The control plane reads the verdict from the report the evaluation leaves and keeps it, with the commit it judged and when, so that a ticket's verdicts can be shown over time.

## Consequences

A ticket that stays open after its pull request merges says why, with evidence, and an owed measurement is settled when it comes due and not when someone remembers. A person still decides to close, so a wrong judgement costs a comment and not a closed ticket.

It costs an agent run for each evaluation, and a read-only token to provision and keep, which is the person's to create and the operator's to protect. The execution environment of a run changes for the first time to include a token, so the executor must give it only to this kind of workload; the token's reach is the risk, and it is reduced by being read-only and by being a different credential from the one that writes.

A verdict is as good as the skill that produces it and the evidence it could read. A ticket whose evidence is out of reach is *cannot judge*, and says what was missing.

What is built, in order, each with its tests first: the workload kind and its verdict contract; reading and keeping the verdict and owed items; the comment on the ticket; queuing after a merge; queuing again while owed; the read-only token's grant to this kind alone. The skill is in place and is not yet released, so the first slice also moves the harness's pin to a release that carries it.
