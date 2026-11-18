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
- Until the executor can run a review as a review, it refuses to run one at all. Run as a ticket it would change code and open a pull request.
- The executor will refuse a verdict that does not pass the review contract, or that names another commit or lens, and a review that changed the working tree. That is the next slice; this decision is recorded now so the contract does not outrun it.

## Consequences

Reviews cost what any workload costs and are bounded the same way. The lens text is data from the pinned Athenaeum release placed in a workload by the control plane, so it is the control plane's job to read it from the repository's pinned release and not from anywhere a pull request can change. A pack with no lens is a decision for where reviews are requested.
