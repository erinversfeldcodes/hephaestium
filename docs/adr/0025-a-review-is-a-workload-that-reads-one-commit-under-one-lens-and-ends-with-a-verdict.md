# 0025. A review is a workload that reads one commit under one lens and ends with a verdict

## Status

Accepted

## Context

A pull request Hephaestium opens is judged by its checks and nothing else. Athenaeum now has the reviewer contract and the first lenses, and a review needs somewhere to run: the same isolation, limits, cost accounting and event stream as any other work, with its answer recorded where the control plane can read it and no agent's own claim of "approved" counts.

- Option 1: The control plane calls a model directly. A second way to run an agent, with its own limits, secrets and cancellation, none of it in the executor.
- Option 2: A review is a workload. The executor already runs one agent in one isolated workspace under limits and records how it ended.
- Option 3: A review is a skill the implementing agent runs on its own work. The author reviewing its own change is what the lenses exist to avoid.

## Decision

We will go with Option 2.

- A workload gains a `review` section beside `revision`: the pull request, the lens's name (the pack), and the lens's text. The lens travels in the message so a review can be run again from the message alone.
- The commit reviewed is the workload's `base_commit`. The workspace is that commit, so what is read is what is named, and the verdict must name the same commit.
- A workload is a ticket's work, a revision, or a review, never two of these. The executor refuses one that is, and refuses a malformed review before it creates anything: a pull request number not above zero, an address not on GitHub, a lens that is not a pack's name (it is used to name a file, so it is letters, digits, `-` and `_`, and starts with a letter or digit), and no lens text.
- The executor runs a review as a review. The agent is told to read the change against the pull request's base branch under the lens, to change nothing, and to write its verdict to `.heph-verdict.json`, with the review contract's schema in the prompt. It delivers nothing and runs no gate.
- The run fails unless the workspace is as it was when the agent started (same commit, no changed or new file, no further edit to a file bootstrap had already changed) and the verdict is JSON that passes the review contract, names the commit under review and names the lens asked for. A verdict that does is kept in the run record, as it passed. An agent's own statement of approval is never read.

## Consequences

Reviews cost what any workload costs and are bounded the same way. The lens text is data from the pinned Athenaeum release placed in a workload by the control plane, so it is the control plane's job to read it from the repository's pinned release and not from anywhere a pull request can change. A pack with no lens is a decision for where reviews are requested.
