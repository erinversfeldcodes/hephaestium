# 0026. A ticket is done when the lenses that block have approved its head commit

## Status

Accepted

## Context

Reviews exist (ADR 0025) and are asked for when a pull request's checks are green. Until their verdicts decide something, they are decoration. The lessons that produced them are the same as for checks (ADR 0024): an agent's judgement of its own work is not enough, a missing answer is not a yes, and a loop that revises forever never ends.

- Option 1: Show the verdicts on the ticket and leave the decision to a person. That is the situation a review was meant to end.
- Option 2: Any non-approving verdict fails the ticket. A reviewer's finding is often something the author can fix in a minute.
- Option 3: A blocking finding is a revision, bounded, and what cannot be fixed fails the ticket.

## Decision

We will go with Option 3.

- Which verdicts count is fixed by the head commit: a verdict is read only for the pull request's head as it is now, so a revision that moves the head starts the checks and the reviews again.
- The order of what matters: a lens that blocks and found a blocking finding is a revision; a lens that blocks and could not review is a failure; a review still running is a wait; every lens that blocks having approved is done. Red is not waited on, as with checks.
- A revision for a blocking finding is the same kind of workload as one for a failing check, spends the same budget (`HEPH_REVISIONS`), and is told the finding (lens, severity, location, evidence, what to revise) as data to read and never as instructions. A finding that is not blocking is not sent.
- A lens that could not review is never an approval: its run failed, timed out, was cancelled or blocked, it completed with no verdict in its record, or its text is not in the release. The ticket is `heph:failed` and says which.
- A lens says in its own front matter whether it blocks. An advisory lens (`blocking: false`) is waited for and reported on the ticket, and nothing it says, or fails to say, stops the change.
- Merging, when on, only follows approval, and still requires GitHub to call the pull request `clean`.

## Consequences

A pull request that reviewers dislike gets a bounded chance to be fixed, and one that cannot be fixed is visible on the ticket with what was found. The cost is the reviews themselves, each a workload, run again after each revision. A revision that satisfies a reviewer and not the code's purpose is possible, so approval is not a person's sign-off. A lens failing to run blocks the ticket until a person looks, which is the safe direction and sometimes an annoying one; retrying a failed review is a later decision. A revision is told the finding in the shape a failed check has, so the prompt says checks where it means a review.
