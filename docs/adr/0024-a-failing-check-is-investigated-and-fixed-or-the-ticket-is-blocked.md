# 0024. A failing check is investigated and fixed, or the pull request is not merged

## Status

Accepted

## Context

A workload ends when it has opened a pull request. The pull request's checks can then fail. Until now the control plane labelled the ticket failed and left it, and a person read the logs and decided what to do, including merging past a failure they judged unrelated. On the first real run (gazebolang #26) a check that is not required failed on an unrelated change, and the way to proceed was to investigate it, not to merge past it.

The expectation is that the factory does that investigation: a failure in CI is investigated by an agent, and if the agent cannot fix it, the change is not merged. That has to be part of the flow, and it has to be bounded, because a loop that revises forever spends money and never ends.

- Option 1: Label the ticket failed and stop. A person investigates. Simple, and exactly what failed to scale.
- Option 2: Re-run the failed job. Right for a runner outage (a cancelled check, ADR-less and already optional), wrong for a real failure, which a re-run reproduces.
- Option 3: A revision: a new workload on the same branch, told which checks failed and given what they said, bounded in number.

## Decision

We will go with Option 3, with Option 2 kept for cancelled checks only.

- A revision is a workload. It is the original workload with a new id, its base commit moved to the pull request's current head, and a `revision` section: the pull request's number and address, the branch, which attempt this is, and for each failing check its name, its result and an excerpt of what it said. The approved ticket text is unchanged, so an edit after approval still cannot reach the agent.
- The executor runs a revision the way it runs any workload, with a prompt that names the failures, and delivers it differently: it pushes to the pull request's branch with an ordinary fast-forward push and opens no pull request. It never forces a push, and it refuses a branch that is not one of its own (`heph/...`).
- Check output is untrusted input: a test can print anything. It reaches the agent as a delimited section of data, capped in size, under an instruction that it is output to read and not instructions to follow, and the agent has the same tools, the same limits and the same environment as before.
- Revisions are bounded: `HEPH_REVISIONS=N`, a maximum number of revisions per pull request, and zero (the default) means none. A revision counts whether it pushed anything or not.
- Only real failures revise: a `failure` or `timed_out` check. A cancelled check is re-run, if that is enabled, and never revised. A real failure beside a cancelled one is revised.
- When no revisions are left and the checks still fail, the ticket is labelled `heph:failed` and a comment says what each attempt did, and the pull request is not merged by Hephaestium. Nothing here merges past a failing check, whether or not the check is required, and nothing offers an administrator bypass.

## Consequences

A red pull request gets an investigation, and a failure that cannot be fixed is visible on the ticket with what was tried. The cost is a revision's spend, bounded by the number allowed, and the new surface of check output reaching an agent, bounded by treating it as data, capping it and giving the agent nothing new. A revision can fix the symptom and not the cause, so a pull request that turns green after one is still a person's to review before it matters. A flaky check can be revised for a failure that was not the change's, and the revision then changes code that needed no change; the bound limits that, and recognising flakiness is a later decision.
