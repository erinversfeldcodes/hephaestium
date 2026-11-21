# 0035. A change is reviewed locally under every lens and pushed only when each has approved it

## Status

Accepted

## Context

A pull request is reviewed only after it has been pushed and its checks are green (ADR 0025, ADR 0026). A blocking finding then costs a revision on a branch that is already public, a fresh round of CI, and a comment trail that records the author's first draft. Everything the lenses would have caught was pushed first.

Reviews are workloads that read one commit under one lens and end with a verdict. Nothing about that needs the commit to be on GitHub: the executor already runs a read-only agent in a workspace and holds it to changing nothing.

- Option 1: Leave reviews after the push.
- Option 2: Review locally before every push, with the same lenses and the same verdict contract, and keep the reviews after the push as a second, independent round.
- Option 3: Review only locally and drop the reviews after the push. It spends less, and it leaves nothing on the pull request for a person to read, and nothing that checks the work with no memory of what the author was told.

## Decision

We will go with Option 2.

- **Round 1, before every push** (the first run's pull request and every revision's push). After the gates pass, the executor chooses the lenses the change needs from its diff against the base, as `heph lenses` does for a pushed pull request, and runs one fresh read-only agent for each. Each reads the local commit under one lens and writes a verdict under the same contract and the same snapshot check as a review after the push.
- **The lens library comes with the workload.** The executor holds no GitHub token before the push, and the lenses depend on what the agent changed, so the control plane puts the pinned release's lens texts in the workload and the executor chooses from them. A lens says in its own front matter whether it blocks and whether it applies to every change, as now.
- **A blocking finding resumes the implementing agent**, told the finding as data to read and never as instructions (ADR 0026). The gates run again on what it changed, and the lenses read the new commit. Nothing is pushed until every lens that blocks has approved the commit it is about to push.
- **What is sent back is what blocks.** A finding that does not block is not sent. It appears in the second round's comments if that round's lens finds it.
- **Round 1 is bounded** by a number of rounds in the workload (`HEPH_PREPUSH_ROUNDS`, default 2), apart from the revisions after a push. A lens that cannot review is no approval, as before. When the rounds are used up and a lens that blocks has not approved, nothing is pushed: the run ends as failed, says what was found, and its workspace is kept.
- **Round 2 is as ADR 0025 and ADR 0026 have it**, run on the pushed commit by fresh agents that comment on the pull request. They are built from the ticket, the commit and the lens, with no GitHub token and no access to the pull request's comments, and nothing from round 1 reaches them, not in their instructions and not in their environment. A test proves that.
- **One account of spend.** The run's cost covers every agent it ran: the implementing agent and each local reviewer. The workload's budget bounds that total, in the budget's own unit, and a run that exceeds it ends without a push. The run record lists each step (the implementation, each round's lens, each revision) with its own cost, so that what a ticket spent on reviewing can be told apart from what it spent on the work.

## Consequences

A pull request arrives having already satisfied every lens that blocks, so its first CI run and its first public reviews are of a better draft, and a revision after the push is for what only CI or a fresh reviewer could find. The price is spend and time: every lens reads the change at least twice, and each local round adds an implementing turn, so review spend more than doubles in the worst case, and a push comes later. A ticket that cannot get past round 1 fails with nothing on GitHub, which loses the pull request as a place to see the attempt; its findings are on the ticket and its workspace is kept. Round 2 agreeing with round 1 is weaker evidence than a first look, but it is still a fresh read by agents that were told nothing, and a disagreement is a signal. A workload's budget now has to be set with the reviewers in mind, because they spend from it.

Not decided here: a limit on what a ticket spends across all its workloads (the implementation, its revisions, the reviews after the push and the judgement of it), which no part of the system enforces today.
