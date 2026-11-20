# 0031. A run brings in the base branch and reconfirms its change

## Status

Proposed

## Context

A run works on the base commit it was given when it was made, or on the pull request's branch as it was. Nothing checks whether the base branch has moved. A revision or a resume therefore runs on stale ground, a pull request is opened against a base that has changed, and a change another run already made, for a different ticket, goes unnoticed: the new pull request duplicates it or conflicts with it. Settle already notices a pull request that is `behind` or has conflicts (it says so on the ticket) and nothing acts on it.

Several things have to be decided together, because each one changes what the others can promise.

- Option 1: Leave it to the repository's own "update branch" button and to the person. No run reconfirms anything, and a duplicate is found by a reviewer, if at all.
- Option 2: Merge the base in mechanically, and fail the run on any conflict. Clean merges are free, but a change that has become unnecessary, or half-done by someone else, is not noticed, and a conflict wastes the run.
- Option 3: Merge the base in, and when something came in that touches the same files, or conflicts, hand it to the agent to resolve and to reconfirm the change against it, before the gates run again and the work is delivered.

## Decision

We will go with Option 3.

- The base is checked at three points: before a revision or a resumed run starts its agent, before a pull request is opened, and when settling finds a green pull request `behind` or with conflicts (which asks for a revision whose first job is this, counted against the revision budget).
- Bringing the base in is a merge commit onto the branch, never a rebase and never a force push. A merge that is clean and brings in nothing that touches a file the change touches is made by the executor with no agent involved: the gates run again on the merged tree, and the description says what came in.
- Anything else (a conflict, or an overlap) is handed to the agent, in the same workload and within its budget: the commits that came in, the files that overlap the change, and the conflicts left in the working tree. The agent resolves them, and reconfirms two things before it goes on: that the ticket's problem still exists on the current base (if another change addressed it, wholly or in part, it says which commit and does not build a duplicate), and that its own change still does what it set out to once what came in is there. Proofs (tests, measurements) are run again on the merged tree and not reused.
- The files a change is judged by (which gates apply, which lenses are asked for) are those that differ from the commit the merge produced, not from the old base, so what came in from the base is not mistaken for the agent's change.
- A run that finds its ticket already resolved by the base makes no change, opens no pull request, and ends saying which commit resolved it. A conflict the agent cannot resolve fails the run and pushes nothing.
- Only pull requests Hephaestium opened (a `heph/` branch of the same repository, ADR 0030) are brought up to date this way. Others are for a person.

## Consequences

A revision, a resume and a first delivery are built on current ground, and a change another run already made is found by the run that would have duplicated it. It costs an agent turn when something overlapping came in, and gates run again after a merge. The executor learns about merging, and the runtime's contract gains what the agent is told (the commits, the overlapping files and the conflicts), which is a change to the protocol to be made once, in this decision's implementation. How a person who disagrees with the agent's judgement that a ticket is already resolved reopens it is left to the resume of ADR 0030.
