# 0023. Intake reads an approval from the ticket and fails closed

## Status

Accepted

## Context

ADR 0020 makes a maintainer's approval of a ticket the only stop before a workload starts, and says it is recorded on the ticket and given by someone with write access. The control plane has to read that decision from GitHub and start a workload only when it stands. Three things in that are not obvious: what an approval is, when it stops standing, and what to do when GitHub cannot say.

- Option 1: An approval is the label being on the ticket. Simple, but the label says nothing about who put it there or when, and an approval that cannot be told from a stale one would start a workload twice.
- Option 2: An approval is a labelling event: who applied the label and when. The most recent one counts, a re-applied label is a new approval, and a workload is started once for each approval time.
- Option 3: Ask a person to confirm on a dashboard. This adds the stop ADR 0020 removed.

## Decision

We will go with Option 2.

- The approval is the most recent `labeled` event for `heph:ready`: its actor must have write access (admin, maintain or write), and its time is the approval time, which the workload records.
- It stands only while no one without write access has edited the ticket since: a rename from the ticket's events, an edit of its body from GitHub's edit history. An edit by an editor GitHub does not name cannot be shown to be a maintainer's, so it voids the approval. A maintainer's own edit does not.
- A workload is started once for each approval time. Removing the label and applying it again is a new approval and starts a new workload.
- Intake fails closed. Anything GitHub or `heph` cannot say, or says an error about, is not an approval, and the ticket is skipped with the reason, so that it is tried again at the next pass and a person can see why not.
- The workload is built by `heph workload` with the labeller as the approver and the label's time as the approval time, so that one piece of logic builds every workload, and the control plane holds no GitHub token: it reads through `gh`.

## Consequences

A ticket cannot be approved by someone who may not, or edited by a stranger after a maintainer approved it, and an approval is something a person can point to in the ticket's history. The cost is several reads of GitHub for each approved ticket on each pass, and a pass that has to wait for GitHub when GitHub is down. A ticket whose labels are moved by hand to look approved is skipped because no labelling event says who did it. The labels still have to be moved as a workload progresses, so that a ticket that has started is not read as ready; until then the rule that an approval starts one workload is what prevents it, and a decision about how labels carry state is a later one.
