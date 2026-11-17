# 0020. A workload stops for a person only to approve its ticket

## Status

Accepted

## Context

The harness Hephaestium grew out of stops for a person after the plan, after every phase commit and at completion. An unattended runtime cannot wait in the middle of a run: it holds a workspace, a process and a budget, and a person who is away leaves all three idle. Every stop also moves the decision from where the evidence is, in the run, to a person reading a summary of it.

- Option 1: Keep the stops: ticket, plan, each phase commit, completion.
- Option 2: Stop for the ticket and, for tickets labelled as needing it, the plan; never at phase commits.
- Option 3: Stop for a person only to approve the ticket, before the workload starts. Nothing else pauses it.

## Decision

We will go with Option 3, and what follows from it.

- A workload for a ticket starts only after a maintainer has approved the ticket. Approval is recorded on the ticket itself, and Hephaestium checks that the person who gave it has write access to the repository, because a maintainer is the only kind of person it takes direction from.
- Nothing else pauses a workload. There is no plan approval and no stop at a phase commit.
- A workload that cannot go on without a decision it cannot make does not wait. It ends as `blocked`, with the question posted on the ticket, and the approval is withdrawn. A maintainer who answers approves the ticket again, which starts a new workload.
- The runtime never merges into a default branch. It opens a pull request, and a maintainer reviews and merges it, as the default branch's ruleset already requires. Review of that pull request is a person's act outside the runtime, not a stop inside it.

## Consequences

A ticket has to be ready for unattended work when it is approved: scoped, with the success criteria and how each will be proven written down, and the ambiguities resolved. There is no later checkpoint to repair a vague one, and a vague ticket will end as `blocked` or as work the maintainer rejects at review. The `scope-request` skill exists to produce that brief. A workload never idles waiting for a person, so a budget and a timeout are enough to bound it. A plan the maintainer would have wanted to veto is caught at the pull request, after the cost of building it has been spent, and a ticket class that is costly to get wrong can be given a plan stop later by a new decision.
