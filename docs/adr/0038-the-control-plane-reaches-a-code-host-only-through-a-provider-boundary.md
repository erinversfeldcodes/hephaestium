# 0038. The control plane reaches a code host only through a provider boundary

## Status

Proposed

## Context

The control plane reads and writes GitHub from many places, by running `gh` and by interpreting what GitHub answers where it uses it. Tickets are GitHub issues (ADR 0003) and every pull request is a GitHub pull request, so the control plane's logic and GitHub's shapes are tangled. Two things now need to be separated: a receiver of webhooks (ADR 0039), which would add one more reader of GitHub's payloads, and a second code host, GitLab, which would have to touch every place below.

**Every call that reaches GitHub, found by searching `control-plane/lib` and `crates` (2026-11-22).** In the control plane, 36 calls go through `ControlPlane.GitHub` (`get`, `get_all`, `text`, `raw`, `graphql`, `write`), in nine modules, outside `github.ex` itself; three more are direct HTTP requests in `review_app.ex`; the Rust side has its own. Grouped by the operation each would become:

*Reading tickets*

- **List the approved tickets of a repository** (1): `Intake.run`.
- **Read a ticket's history: who labelled, who renamed, who reopened, and when** (2): `Intake.consider`, `Gaps.reopened_since`.
- **Read a person's permission on a repository** (1): `Intake.writer`.
- **Read who edited a ticket's body, from its edit history** (1, the only GraphQL call): `Intake.not_edited_since`.
- **Read one ticket: its state** (2): `Evaluations.open`, `Gaps.open`.
- **List a repository's open tickets with their sub-issue progress** (1): `Evaluations.epics_of`.
- **Read a ticket's sub-tickets** (1): `Evaluations.roll_up`.
- **Read a ticket's comments** (1): `Gaps.answered`.

*Writing tickets*

- **Create a ticket, make it a sub-ticket, make it wait for another** (3): `Gaps.create`, and `Gaps.link` twice (the sub-issue and the `blocked_by` dependency).
- **Add a label, remove a label, comment on a ticket** (1 call, three operations): the one writer in `Tickets` (`add_label`, `remove_label`, `comment` all go through `Tickets.write/3`).
- **Comment on a change** (1): `PullRequestComments.post`.

*Reading changes (pull requests) and their checks*

- **Read a change: state, merged, head, base, how it can merge** (4): `Settle.stale?`, `Settle.settle`, `Evaluations.look_for_merge`, `Intake.plan`.
- **Read a change's files** (2): `Reviews.changed_files`, `Settle.base_moved`.
- **Compare a head with its base** (1): `Settle.base_moved`.
- **Read what the base branch requires of a change** (1, the ruleset): `Settle.verdict`.
- **Read a head's checks** (1, the check runs of a commit): `Settle.verdict`.
- **Read a failing check's log** (1, the log of a GitHub Actions job): `Settle.excerpt`.
- **Read a check's annotations** (1): `Settle.timed_out`.
- **Read the reviews on a change** (1): `ReviewApproval.ensure`.

*Acting on changes*

- **Merge** (1): `Settle.put_merge`.
- **Rerun a check** (1, a GitHub Actions job): `Settle.act`.
- **Approve** (1): `ReviewApproval.submit`, with the review App's token.

*Reading repository content at a revision*

- **Read a file at a ref, or list a directory at a ref** (6): `Prepush.read`, `Reviews.manifest`, `Reviews.with_universal`, `Reviews.pack_names`, `Reviews.lens_texts`, `Evaluations.skill`. Five read a file; `pack_names` lists a directory. All but `Reviews.manifest` read the Athenaeum repository (skills and lenses), not the managed repository.

That is 36. Beyond them:

- **Authenticating as an App** (3 requests, in `ReviewApp.mint`): the App's identity, its installation on a repository, and an installation token. These are not a repository operation. They are how one GitHub implementation proves who it is.
- **`heph workload`** (Rust, `crates/heph/src/workload.rs`, five reads through `crates/heph/src/github.rs`): the repository, the tip of its base branch, the ticket, who is signed in, and what blocks the ticket. The control plane runs it from `Intake.build`, so it is a GitHub path out of the control plane that is not an Elixir call. The same `github.rs` also serves the baseline settings check and apply (`crates/heph/src/baseline.rs`, `apply_settings.rs`), which no control-plane path uses.
- **The executor's delivery** (Rust, `crates/executor/src/deliver.rs`): `git push` with `gh auth git-credential` as its credential helper, then `gh pr create` or `gh pr edit`. And the read-only token the executor hands to review, evaluation and questioning agents as `GH_TOKEN`, whose prompts tell the agent to use `gh issue view` and `gh api`. These run on the developer's machine (ADR 0037).

**Where GitHub's own shapes are interpreted.** The control plane reads fields it was handed, not a type: `mergeable_state` (`"behind"`, `"clean"`, `"dirty"`, `"blocked"`, `"draft"`, and `nil` or `"unknown"` while GitHub computes it) in `Settle`; `head.sha`, `head.ref` and `base.ref`; the `pull_request` key that tells an issue from a pull request; `labels[].name`; `sub_issues_summary` (`Evaluations.finished_epic?`). A second host would have to produce all of these in GitHub's spelling, or every one of them is rewritten.

**How it is tested.** Control-plane tests stand in for GitHub with a fake `gh` program (`control-plane/test/support/stand_in_github.ex`), which answers recorded shapes, so a second host would need a second fake of a different kind.

**What is known about GitLab, and what is not.** It has issues, merge requests, labels, approvals and pipelines with jobs, and its webhooks are called Issue, Merge Request, Pipeline, Note and Push hooks; those are the owner's list. Anything finer (how its labels and label events are named, whether sub-items and "blocked by" links exist on every plan, how a merge request says it can merge, how an approval is made and counted, how nested groups name a project, whether a job's log and annotations are reachable the way GitHub's are) is not established here and is marked to be verified below. No part of the design below may rest on it unverified.

The options:

- Option 1: Leave the calls where they are, and add a second host by teaching each module to ask which host it is dealing with. A second host touches every module in the list and its tests, and each module's logic stays written in one host's words.
- Option 2: A `Host` behaviour in the control plane with the operations the control plane needs, one implementation for each host, and provider-neutral events that a host emits. The logic is written once, in the control plane's terms; a host is the only place that knows its own endpoints, field names and quirks.
- Option 3: Put a generic adapter library or an external gateway between the two. It adds a service and a dependency, and the operations here (ticket approval from a label's actor and time, the check settling rule of ADR 0024, the review App's approval) are this project's own and no library would know them.

## Decision

We will go with Option 2.

**A `Host` behaviour.** It lives in the control plane, with one module for each host, chosen by the repository's configuration. A repository is named with its host, and a bare name is GitHub, so no existing configuration changes meaning. A workload's record keeps the host beside the repository, defaulting to GitHub for every row that exists.

Its operations are the groups above, named for what the control plane means by them:

- **Tickets:** `approved_tickets(repository)`; `ticket(repository, number)` with its state, labels and relations (parent, sub-tickets, what blocks it); `ticket_history(repository, number)` as the labelled, unlabelled, renamed, closed and reopened events with their actor and time; `ticket_edits(repository, number)` with who edited; `open_epics(repository)`; `comments(repository, number)`; `permission(repository, login)` as one of admin, maintain, write, read or none; `create_ticket`, `link_sub_ticket`, `block_ticket`; `add_label`, `remove_label`, `comment_on_ticket`.
- **Changes:** `change(repository, number)` with its state, merged or not, head, base and a normalised mergeability (clean, behind, blocked, conflicted, draft or unknown, so that `Settle` no longer reads GitHub's strings); `change_files`; `compare(repository, base, head)`; `comment_on_change`; `merge`; `reviews`; `approve(repository, number, commit, body)`, answering `:approved`, `:submitted`, `:disabled` or `:unsupported`.
- **Checks:** `required_checks(repository, branch)`, `checks(repository, commit)` as a list of `{id, name, state, conclusion, required}`, `check_log`, `check_annotations`, `rerun_check`.
- **Content:** `file(repository, path, ref)` and `directory(repository, path, ref)`.
- **Which operations a host has.** `capabilities/0` returns the set a host supports (sub-tickets, blocking links, approval, check rerun, check annotations). A capability a host lacks is not an error to be guessed at: the control plane that needs it reads the set, and a feature without its capability is off for that host, with a reason, in the way intake already fails closed (ADR 0023).
- **How a host proves who it is is the host's own business.** The App token flow in `ReviewApp`, `gh`'s own login, and any future GitLab token are inside an implementation, not in the behaviour. Approval is an operation of the host and the identity it is made under (the review App, ADR 0033) belongs to the implementation.
- **No payload leaks out.** An operation returns the control plane's own structures (a ticket, a change, a check), never a host's JSON. `mergeable_state` and `sub_issues_summary` are read in the GitHub implementation only.

**Events.** A host module also turns what its code host sends (ADR 0039) into provider-neutral events, which are Elixir structs and not a wire contract, because nothing outside the control plane reads them. Each carries the host, the repository, the number of the ticket or change, the actor, the time, and the code host's delivery id, and carries no state of the code host: a handler reads the current state through the behaviour, so an event can be late, repeated or out of order and the answer is the same, and an approval is still read from the ticket's history and never from an event (ADR 0023). The events:

- `TicketApproved`, `TicketEdited`, `ChecksCompleted`, `ChangeUpdated`, `ChangeMerged`, `BaseMoved`, `CommentAdded`: the seven the design needs.
- Three more, which the current jobs show are also needed: `TicketClosed` (an epic is rolled up when its last sub-ticket closes), `TicketReopened` (a person reopening what Hephaestium closed) and `ChangeClosed` (a change closed without merging, which settling reads as abandoned).
- Two causes rather than events: a label being removed is a `TicketEdited` with the cause `unapproved`, and a review being submitted or dismissed is a `ChangeUpdated` with that cause.
- `CommentAdded` says whether the comment is on a ticket or on a change, because GitHub sends both as the same kind of event.

**What does not map one to one, and how the behaviour holds it.**

- **The review App's approval** is an approving review on GitHub counted by the ruleset (ADR 0033). GitLab has merge-request approvals, whose rules, and whether an author can approve, are configured in the project (to be verified). The operation is `approve`; a host for which no approval can count answers `:unsupported`, and a change on it is reviewed in comments and merged by a person, as ADR 0033 already has it without the App.
- **Labels carry state** (ADR 0023 reads the actor and time of the label's application). GitLab has labels and records label events with a user and a time (to be verified). A host whose label events do not name the actor cannot be a host for approval, and says so by not being one.
- **Sub-tickets and blocking links.** GitHub has both natively (ADR 0003). GitLab's hierarchy of work items and its "blocked by" links depend on the plan (to be verified). Without sub-tickets, epics are off on that host, and without blocking links a ticket's blockers are not read.
- **Checks.** GitHub's check runs, with the ruleset's required checks, map onto GitLab's pipelines and jobs and its "pipelines must succeed" setting (to be verified). A check log and a rerun are, on GitHub, those of a GitHub Actions job; a check from another provider has neither, and the behaviour already treats that as not rerunnable. The GitLab equivalents (job logs, retrying a job) are to be verified.
- **Repository names.** GitLab project paths can nest in groups (to be verified), so a repository is an opaque string to the control plane, as `owner/name` already is stored.
- **Where the Athenaeum is read from** is not the managed repository's host. A GitLab-hosted repository that pins an Athenaeum release on GitHub reads it from GitHub, so `file` and `directory` take a host as well as a repository, or the Athenaeum is a configured source of its own. Which is not decided here.

**The order of work.**

1. **Define the behaviour and the data types** from the groups above, with no caller changed.
2. **Extract with no change of behaviour.** The GitHub implementation takes over the existing function bodies, and the existing interpretation of GitHub's fields moves into it. The callers are moved one module at a time. The whole existing test suite passes without change to what it asserts, because the stand-in `gh` is now the GitHub implementation's concern and the modules above no longer know it. A guard fails the build when `ControlPlane.GitHub`, `gh` or a `repos/` endpoint is referenced outside the GitHub implementation, and is shown failing on a planted violation first (as AGENTS.md asks of a new guard). Tests of the logic then use a stand-in host that answers with the control plane's own structures, which is simpler than the fake `gh`.
3. **Emit events**, with the receiver of ADR 0039, so that polls and webhooks call the same handlers.
4. **Add a second implementation** only when a ticket asks for it, and only after the unverified points above are verified against the host's documentation and a real project. A second implementation may need its own decision, for a tracker mapping that ADR 0003 does not give.

## Consequences

The control plane's logic is written once and a host is a module, so adding GitLab is writing one implementation against a set of operations that has been listed from what the code does, and it is tested against the same behaviour. A change to GitHub's payloads lands in one place. The webhook receiver emits the same events whichever host sent them. The price: a behaviour with twenty-seven operations to keep coherent, a normalisation layer that can hide a distinction a host makes (mergeability and checks are the likeliest), a data migration for the host column, and a period during which the code is being moved with nothing visible to gain except the guard.

The capability set costs a feature on a host that lacks it, which is the honest alternative to an operation that quietly does nothing. A host that cannot give a label's actor and time cannot be a host for approval (ADR 0023), and that is a limit of the design, not something it can paper over.

What this does not remove: the Rust side. `heph workload` and `heph baseline` read GitHub through `gh` and are GitHub-specific, and the executor's delivery pushes and opens pull requests with `gh`. The control plane's boundary leaves them as they are, and a second host has to change them as well: the `heph workload` path in particular, because the control plane runs it on every approval. Whether `heph` gets a provider trait of its own, or the control plane builds the workload from what the behaviour returns, is not decided here and it is the largest piece of GitHub-specific code the boundary leaves.

Not decided here:

- The GitLab implementation, and everything about GitLab marked to be verified.
- Where the Athenaeum is read from for a repository on another host.
- The Rust side, as above, and the agents' own read-only `gh` access (the prompts of review, evaluation and questioning agents tell the agent to use `gh`).
- Whether one repository may be on one host and its tickets on another, which is assumed not.
- The repository configuration's exact spelling for a host.
