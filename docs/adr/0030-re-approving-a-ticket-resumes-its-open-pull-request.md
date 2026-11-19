# 0030. Re-approving a ticket resumes its open pull request

## Status

Accepted

## Context

A ticket that ended `heph:failed` or `heph:done` with its pull request still open, and was labelled `heph:ready` again, got a new workload built from the ticket. Its branch name is the old one, so its push cannot succeed, and the agent's run is wasted. The chain of workloads already knows the pull request. This is the real case on gazebolang (a good change held up by a flaky check, with no way to pick it up once the check was fixed), and the roadmap's `heph run --resume`, which delivers an existing workspace without running the agent again, is a different thing.

- Option 1: Leave it: a person closes the old pull request and starts again. The work is lost and the ticket's history with it.
- Option 2: Let a new workload push to the old branch. It would start from the ticket and not from what the pull request now holds, and could overwrite review.
- Option 3: A re-approval resumes the pull request: settling looks at it again as it is now.

## Decision

We will go with Option 3.

- Intake, for a ticket a maintainer labelled `heph:ready` with no edit since (ADR 0020, ADR 0023), looks for the latest workload that completed with a pull request in its record and is not a review. If that pull request is open and is a `heph/` branch of the same repository, it is resumed. With no such workload, intake builds a new one, as before.
- A merged pull request, a closed one, one the runtime did not make, and one GitHub cannot be asked about are skipped, with the reason. A new run would collide with the old branch, so a person decides.
- Resuming makes that workload unsettled, so settling judges the pull request as it is now: its checks, its reviews at the head commit, and a revision if the checks really fail. The ticket's labels become `heph:running`, whichever of `heph:ready`, `heph:failed`, `heph:done` and `heph:blocked` it has are removed (and none it does not have is asked to go), and a comment says who resumed which pull request.
- A resume is the maintainer's authorisation to spend again, so it grants a fresh `HEPH_REVISIONS`. The revisions used are counted from the attempt the pull request had reached at the resume (`budget_base`), which later revisions inherit. Revision numbers keep counting up, so the history reads in order, and a comment says "revision k of N" counted from the resume.
- An approval resumes once: the approval's time is recorded on the resumed workload (`resumed_for`), as it is on a workload started from it.
- A green pull request whose reviews approved it is settled done again and nothing is spent.

## Consequences

A pull request can be picked up after its checks, or the preview they ran on, are fixed, without a run that cannot deliver. The cost is that the maintainer's re-approval is the only brake on repeated spending: each resume allows another set of revisions. A pull request that was merged or closed needs a person. The `heph run --resume` of delivering an existing workspace is still open.
