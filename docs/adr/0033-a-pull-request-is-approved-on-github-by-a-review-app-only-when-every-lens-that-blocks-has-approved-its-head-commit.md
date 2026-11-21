# 0033. A pull request is approved on GitHub by a review App only when every lens that blocks has approved its head commit

## Status

Accepted

## Context

The default branch requires one approving review (ADR 0007), and the maintainer bypasses it on every merge, because the maintainer is also the author and GitHub does not let an author approve their own pull request. Hephaestium's reviews (ADR 0025) and their rule for done (ADR 0026) say when a pull request has been read by every lens that blocks and found sound, but that verdict is only a comment on the pull request, so GitHub's own rule never sees it. The result is that the review Hephaestium exists to do cannot satisfy the rule it exists beside, and every merge needs the bypass.

A review from a GitHub App counts toward the rule only if the App can write to the repository: with only the pull-request permission its approval is recorded and does not count; with the repository contents permission as well, the review decision becomes approved.

- Option 1: Keep the verdicts as comments and keep bypassing the rule on every merge.
- Option 2: Remove the required review for pull requests Hephaestium opens, and rely on the required checks.
- Option 3: A review App, separate from the identity that authors pull requests, submits an approving review on GitHub when, and only when, every lens that blocks has approved the head commit.

## Decision

We will go with Option 3.

- The approval is submitted by a GitHub App of its own (`HEPH_REVIEW_APP_ID`, `HEPH_REVIEW_APP_KEY_FILE`), never by the account that authors the pull request. Without both settings nothing is submitted and a pull request is as it was: reviewed in comments, merged by a person.
- It is submitted when the reviews of the head commit say `approved` under the rule of ADR 0026: every lens that blocks approved this commit. A lens that found something, could not review, is still running, was never asked for or has no text means no approval. One approval out of five is no approval.
- Only an approval is ever submitted. A finding is a comment and a revision as before; a request for changes would outlive the commit it was made on and block the revision's own pull request. A new push dismisses an earlier approval (the rule requires approval after the last push), so an approval never stands for a commit no lens read.
- The approval names the commit it was made on, and is submitted once for a pull request and a head commit: GitHub is asked first whether the App has already approved that commit.
- The key stays in a file the operator names, outside any repository, and is read when a token is needed. The control plane signs a short-lived assertion with it, exchanges it for an installation token for the one repository, keeps that token in memory until shortly before it expires, and hands it to `gh` through the environment for the one call. It is never logged, never put in an argument and never written to the database.
- A failure to submit is not an approval and not a failure of the ticket: the pull request waits and the submission is tried again on the next pass.
- Merging stays as ADR 0026 says: only after approval, and only when GitHub itself calls the pull request `clean`. Approving a pull request is not merging it, and the pass that submits the approval waits, because the pull request it read before the approval still reads as blocked.

## Consequences

A pull request that every lens approves can merge without the maintainer's bypass, and the maintainer's approval is no longer what stands between a green pull request and the default branch. That makes the review lenses the gate, so a weak lens is now a weak gate; the rule for done already depended on them, and what changes is that GitHub enforces it. The App needs the repository contents permission to be counted as a reviewer, which also lets it push, so its key is as sensitive as a deploy key, and it must be used only to submit reviews. This reverses the earlier position that nothing Hephaestium says is submitted as a review. The bypass in ADR 0007 is not removed by this: a person may still merge a pull request no lens has read. A repository that wants a person's approval as well can raise the required count to two.
