# 0038. The control plane reaches a tracker and a code host only through two boundaries

## Status

Proposed

## Context

The control plane reads and writes GitHub from many places, by running `gh` and by interpreting what GitHub answers where it uses it. Tickets are GitHub issues (ADR 0003) and every change is a GitHub pull request, so the control plane's logic and GitHub's shapes are tangled. Three things now need to be separated: a receiver of webhooks (ADR 0039), which would add one more reader of GitHub's payloads; a second code host, GitLab; and a second tracker, Jira. The owner has decided that a ticket may in future come from Jira while the repository actions (changes, checks, merges, content) are in GitLab, so a tracker and a code host are two systems that may differ, and the boundary must not assume they are one.

**Every call that reaches GitHub, found by searching `control-plane/lib` and `crates` (2026-11-22).** In the control plane, 36 calls go through `ControlPlane.GitHub` (`get`, `get_all`, `text`, `raw`, `graphql`, `write`), in nine modules, outside `github.ex` itself; three more are direct HTTP requests in `review_app.ex`; the Rust side has its own. Grouped by the operation each would become, and by which system the operation belongs to. 13 of the 36 are the tracker's and 23 are the code host's.

*The tracker: tickets (13)*

- **List the approved tickets of a repository** (1): `Intake.run`.
- **Read a ticket's history: who labelled, who renamed, who reopened, and when** (2): `Intake.consider`, `Gaps.reopened_since`.
- **Read who edited a ticket's body, from its edit history** (1, the only GraphQL call): `Intake.not_edited_since`.
- **Read one ticket: its state** (2): `Evaluations.open`, `Gaps.open`.
- **List a repository's open tickets with their sub-issue progress** (1): `Evaluations.epics_of`.
- **Read a ticket's sub-tickets** (1): `Evaluations.roll_up`.
- **Read a ticket's comments** (1): `Gaps.answered`.
- **Create a ticket, make it a sub-ticket, make it wait for another** (3): `Gaps.create`, and `Gaps.link` twice (the sub-issue and the `blocked_by` dependency).
- **Add a label, remove a label, comment on a ticket** (1 call, three operations): the one writer in `Tickets` (`add_label`, `remove_label`, `comment` all go through `Tickets.write/3`).

*The code host: changes, checks and content (23)*

- **Read a person's permission on a repository** (1): `Intake.writer`. It is in the code host's group because it is the repository's own permission, which is what ADR 0023 asks of an approver.
- **Read a change (pull request): state, merged, head, base, how it can merge** (4): `Settle.stale?`, `Settle.settle`, `Evaluations.look_for_merge`, `Intake.plan`.
- **Read a change's files** (2): `Reviews.changed_files`, `Settle.base_moved`.
- **Compare a head with its base** (1): `Settle.base_moved`.
- **Read what the base branch requires of a change** (1, the ruleset): `Settle.verdict`.
- **Read a head's checks** (1, the check runs of a commit): `Settle.verdict`.
- **Read a failing check's log** (1, the log of a GitHub Actions job): `Settle.excerpt`.
- **Read a check's annotations** (1): `Settle.timed_out`.
- **Read the reviews on a change** (1): `ReviewApproval.ensure`.
- **Comment on a change** (1): `PullRequestComments.post`.
- **Merge** (1): `Settle.put_merge`.
- **Rerun a check** (1, a GitHub Actions job): `Settle.act`.
- **Approve** (1): `ReviewApproval.submit`, with the review App's token.
- **Read a file at a ref, or list a directory at a ref** (6): `Prepush.read`, `Reviews.manifest`, `Reviews.with_universal`, `Reviews.pack_names`, `Reviews.lens_texts`, `Evaluations.skill`. Five read a file; `pack_names` lists a directory. All but `Reviews.manifest` read the Athenaeum repository (skills and lenses), not the managed repository.

That is 36. Beyond them:

- **Authenticating as an App** (3 requests, in `ReviewApp.mint`): the App's identity, its installation on a repository, and an installation token. These are not an operation of either system. They are how one GitHub implementation proves who it is.
- **`heph workload`** (Rust, `crates/heph/src/workload.rs`, five reads through `crates/heph/src/github.rs`): the repository, the tip of its base branch, the ticket, who is signed in, and what blocks the ticket. The control plane runs it from `Intake.build`, so it is a GitHub path out of the control plane that is not an Elixir call. It mixes the two systems: the ticket and its blockers are the tracker's, the repository and its base are the code host's. The same `github.rs` also serves the baseline settings check and apply (`crates/heph/src/baseline.rs`, `apply_settings.rs`), which no control-plane path uses.
- **The executor's delivery** (Rust, `crates/executor/src/deliver.rs`): `git push` with `gh auth git-credential` as its credential helper, then `gh pr create` or `gh pr edit`. And the read-only token the executor hands to review, evaluation and questioning agents as `GH_TOKEN`, whose prompts tell the agent to use `gh issue view` and `gh api`. These run on the developer's machine (ADR 0037).

**Where the two systems meet today, because one system is both.** Few places use both, and they are where a split costs something:

- A change belongs to a ticket by a record the control plane keeps: the executor's pull-request event is stored in the workload's record, and `Workloads.pull_request_workload` and `pull_request_number` find the change from the ticket (`Intake.plan`). The branch is named `heph/<ticket number>-<slug>` (`branch_for` in `crates/executor/src/deliver.rs`), and intake accepts a change as the runtime's only if its branch starts with `heph/` and is in the same repository (`Intake.resumable`). The change's description ends with `Closes #<number>` or `Refs #<number>` (`body` in the same file), which on GitHub closes the ticket when the change merges (ADR 0029).
- Settling reads a change and then writes the ticket (`Settle.settle`, then `Tickets.settled`); the judgement after a merge reads the change (`Evaluations.look_for_merge`) and then the ticket (`Evaluations.open`); the ticket's status comment names the change's address (`Tickets.pull_request`).
- The workload's contract has the ticket's `number` as an integer (`proto/hephaestium/v1alpha1/workload.proto`), and the branch name and the description's reference are built from it.

**Where GitHub's own shapes are interpreted.** The control plane reads fields it was handed, not a type: `mergeable_state` (`"behind"`, `"clean"`, `"dirty"`, `"blocked"`, `"draft"`, and `nil` or `"unknown"` while GitHub computes it) in `Settle`; `head.sha`, `head.ref` and `base.ref`; the `pull_request` key that tells an issue from a pull request; `labels[].name`; `sub_issues_summary` (`Evaluations.finished_epic?`). A second system would have to produce all of these in GitHub's spelling, or every one of them is rewritten.

**How it is tested.** Control-plane tests stand in for GitHub with a fake `gh` program (`control-plane/test/support/stand_in_github.ex`), which answers recorded shapes, so a second system would need a second fake of a different kind.

**What is known about GitLab, and what is not.** It has issues, merge requests, labels, approvals and pipelines with jobs, and its webhooks are called Issue, Merge Request, Pipeline, Note and Push hooks; those are the owner's list. Anything finer (how its labels and label events are named, how a merge request says it can merge, how an approval is made and counted, how nested groups name a project, whether a job's log and annotations are reachable the way GitHub's are) is not established here and is marked to be verified below.

**What is known about Jira, and what is not.** With reasonable confidence, from general knowledge and not from this repository or its documentation: a Jira issue belongs to a project and is named by a key made of the project's key and a number (`PROJ-123`), which is text and not an integer; an issue has labels, which are plain strings; issues can be linked, and the default link types include "blocks" with its inverse; there are issue types for epics and sub-tasks, so a hierarchy exists; an issue has a changelog that records who changed which field and when; an issue has comments with an author and a time; Jira can send webhooks for issue and comment events; and permissions are granted per project. Everything finer is to be verified, and the design below must not rest on it:

- how an epic's children are related in each kind of project, and how a hierarchy is read;
- whether a label's addition is in the changelog with its author and time in the form intake needs (ADR 0023);
- whether the changelog separates an edit of the description from other edits, and names the editor;
- whether an identity can be read as something that maps to a code-host login (the form of an account id, and whether an email address is visible);
- whether a user's permission in a project can be read by an integration that is not that user;
- how a webhook is registered, signed and retried, and how Jira Cloud differs from Jira Data Center in each;
- how an integration authenticates, and the rate limits;
- how a Jira issue and a branch or merge request on GitLab are linked by Atlassian's or GitLab's own integrations (for example by an issue key in a branch name).

The options:

- Option 1: Leave the calls where they are, and add a second system by teaching each module to ask which system it is dealing with. A second system touches every module in the list and its tests, and each module's logic stays written in one system's words.
- Option 2: One `Host` behaviour for both tickets and changes, one implementation per system. The logic is written once, and a ticket and its change are assumed to be on the same system. A Jira ticket with GitLab changes would then need an implementation of the pair, and each tracker would be written against each code host.
- Option 3: Two behaviours in the control plane, a `Tracker` for tickets and a `CodeHost` for changes, checks, merges and content, with provider-neutral events from each, and a repository configured as a pair. A system that is both (GitHub, GitLab) implements both. The two groups above share almost nothing: 13 call sites are tickets, 23 are the code host's, and the places where they meet are the few listed.
- Option 4: Put a generic adapter library or an external gateway between the two. It adds a service and a dependency, and the operations here (approval from a label's actor and time, the check settling rule of ADR 0024, the review App's approval) are this project's own and no library would know them.

## Decision

We will go with Option 3.

**Two behaviours, each in the control plane, one module per system.**

`Tracker`:

- **Reading:** `approved_tickets(project)`; `ticket(project, key)` with its state, labels and relations (parent, sub-tickets, what blocks it); `ticket_history(project, key)` as the labelled, unlabelled, renamed, closed and reopened events, each with its actor and time; `ticket_edits(project, key)` with who edited; `open_epics(project)`; `comments(project, key)`.
- **Writing:** `create_ticket`, `link_sub_ticket`, `block_ticket`; `add_label`, `remove_label`, `comment_on_ticket`; and `close_ticket`. On GitHub a merged change closes its ticket by the keyword in its description (ADR 0029). That does not cross systems, so for a repository whose tracker is not its code host the control plane closes the ticket itself, through this operation, when its rule says to. For a repository with GitHub for both it is not called, so nothing changes there.
- **Who may approve is not an operation of the tracker** under the recommendation below. The tracker says who acted (the actor of the label, as an identity of its own system), and the approval rule is the control plane's.

`CodeHost`:

- **Changes:** `change(repository, number)` with its state, merged or not, head, base and a normalised mergeability (clean, behind, blocked, conflicted, draft or unknown, so that `Settle` no longer reads GitHub's strings); `change_files`; `compare(repository, base, head)`; `comment_on_change`; `merge`; `reviews`; `approve(repository, number, commit, body)`, answering `:approved`, `:submitted`, `:disabled` or `:unsupported`.
- **Checks:** `required_checks(repository, branch)`, `checks(repository, commit)` as a list of `{id, name, state, conclusion, required}`, `check_log`, `check_annotations`, `rerun_check`.
- **Content:** `file(repository, path, ref)` and `directory(repository, path, ref)`.
- **The repository's own permission:** `permission(repository, login)` as one of admin, maintain, write, read or none.

That is thirteen operations for the tracker and fifteen for the code host.

**Capabilities.** Each behaviour has `capabilities/0` returning the set its system supports (for a tracker: sub-tickets, blocking links, a label's actor and time in its history; for a code host: approval, check rerun, check annotations). A capability a system lacks is not an error to be guessed at: the control plane reads the set, and a feature without its capability is off for that system, with a reason, in the way intake already fails closed (ADR 0023).

**No payload and no credential leaks out.** An operation returns the control plane's own structures (a ticket, a change, a check, an actor), never a system's JSON. `mergeable_state` and `sub_issues_summary` are read in the GitHub implementation only. How a system proves who it is (the App token flow of `ReviewApp`, `gh`'s own login, a GitLab or Jira token) is inside an implementation. A repository with two systems has two identities and two sets of credentials, and the review App of ADR 0033 stays the code host's and is used only for approval.

**A repository is configured as a pair.** The configuration names where a repository's tickets live and where its code lives, each as a system and a name in it. A bare name is GitHub for both, the same repository for each, so no existing configuration changes meaning. A workload's record keeps both: the tracker's system and project, the ticket's key as text, and the code host's system and repository. Rows that exist take GitHub for both and the ticket's number as its key. The exact spelling of the configuration is not decided here.

**How a ticket in one system names a change in the other.** The options:

- Link option A: by the branch name. The branch the executor makes already carries the ticket's number (`heph/<number>-<slug>`), so with a Jira key it would carry the key. Cost: the link is read by parsing a name a person can rename, it exists only for changes the runtime made, and what an integration of either system does with a key in a branch name is not established (to be verified).
- Link option B: by a reference in the change's description. It is `Refs`/`Closes` today. Cost: a person with edit rights on the change can change it, so it cannot be trusted as authority, and the closing keyword does nothing across systems; but it is visible to a person reading either side.
- Link option C: stored by the control plane. It is what exists: the workload's record has the ticket's key and the change's address from the executor's own event, and `Workloads.pull_request_workload` and `pull_request_number` find one from the other. Cost: the link exists only for what the control plane did, so a change opened by hand for a ticket is not linked, and the two systems do not know the link unless something is written to them.

We will go with Option C as the link the control plane acts on, and write A and B as the visible, redundant links: the branch carries the ticket's key, the change's description refers to the ticket, and the ticket's status comment names the change's address. None of the three is read back as authority, except that intake still recognises a runtime change by its branch prefix, as it does now. The recommendation rests on evidence found in the code (the record already is the link), not on anything about Jira or GitLab, and it is the one a person cannot edit. The ticket's key being text, the executor's contract needs it beside the integer number, and the branch and the description are built from it; that is an additive contract change, owned by the build.

**Approval authority.** ADR 0023 makes the approval the most recent labelling event for the ready label, by an actor who has write access, and reads that access from the repository. With a separate tracker the actor is an identity in the tracker (a Jira account), and nothing yet says that person may change the repository. The options:

- Authority option A: the tracker's own permission. The approver must hold a permission in the ticket's project. Cost: the authority becomes "may edit tickets in this project", which is not "may change this repository", so a person who may label an issue could start work on code they cannot touch. The work still arrives as a change to be reviewed and merged, but ADR 0023's rule is replaced for this case. Whether an integration can read another user's permission is to be verified.
- Authority option B: the code host's permission, with the tracker identity mapped to a code-host login. The approver's tracker identity is mapped to a login, and the check of ADR 0023 is run unchanged on that login through `CodeHost.permission`. Cost: the mapping. If it is inferred (by an email address) it depends on facts not established (whether the tracker shows an address and the code host lets one be matched), and a wrong match is a wrong authority. If it is declared by the operator, a person is added to a list.
- Authority option C: an allowlist of tracker identities the repository's configuration names, and nothing else. Cost: it is maintained by hand and drifts from who may really change the repository; it is verifiable without any outside fact, and it fails closed; but it is a different authority from the one ADR 0023 states.

We will go with option B with a declared mapping, and recommend it for the owner to accept. The configuration names, for each repository, the tracker identities that may approve and the code-host login each stands for. An approval is a labelling event whose actor has a declared login and whose login holds write access on the code host's repository. An actor with no declared login, or whose permission cannot be read, is not an approval: the ticket is skipped with the reason and tried again at the next pass, as intake already does. It keeps ADR 0023's rule intact, "a maintainer is a person with write access", and adds only the step that turns a tracker identity into the code-host login that rule needs. What depends on unverified facts: nothing in the declared form. The inferred form (email) would, and is not recommended. For a repository with GitHub for both there is no mapping, the actor is the login, and ADR 0023 stands as written.

**Does ADR 0023 need to be superseded?** Not for the GitHub-for-both case, which this changes in no way. For a separate tracker with option B, ADR 0023's rule is extended by the mapping step, and that extension is recorded here; ADR 0023's text is not contradicted. If the owner chooses option A or C instead, the rule that the approver has write access to the repository is replaced for that case, and a new ADR that supersedes ADR 0023 for it is needed, because an accepted ADR is read-only. Either way the rest of ADR 0023 (an approval is read from the ticket's history and not from an event, a voiding edit, fail closed, one workload for each approval time) is expected to hold for a tracker whose history gives an actor and a time, and a tracker whose history does not cannot be a tracker for approval.

**Events.** Each system emits its own events, as Elixir structs and not a wire contract, because nothing outside the control plane reads them. Each carries its source (the tracker or the code host and the system's name), the repository or project, the key or number of the ticket or change, the actor, the time, and the delivery id; none carries the system's state, so a handler reads the current state through the behaviour and an event can be late, repeated or out of order. An approval is read from the ticket's history and never from an event (ADR 0023).

- **From the tracker:** `TicketApproved`, `TicketEdited` (the label being removed is this, with the cause `unapproved`), `TicketClosed`, `TicketReopened`, `CommentAdded` on a ticket.
- **From the code host:** `ChecksCompleted`, `ChangeUpdated` (a review submitted or dismissed is this, with that cause), `ChangeMerged`, `ChangeClosed` (without merging, which settling reads as abandoned), `BaseMoved`, `CommentAdded` on a change.
- **Three more than the seven the design started with** (`TicketApproved`, `TicketEdited`, `ChecksCompleted`, `ChangeUpdated`, `ChangeMerged`, `BaseMoved`, `CommentAdded`): `TicketClosed` (an epic is rolled up when its last sub-ticket closes), `TicketReopened` and `ChangeClosed`. GitHub sends a comment on a ticket and one on a change as the same kind of event, so `CommentAdded` says which it is.

**The order of work.**

1. **Define the two behaviours and their data types** from the groups above, with no caller changed.
2. **Extract GitHub behind both, with no change of behaviour.** The GitHub implementation takes over the existing function bodies and the existing interpretation of GitHub's fields, as a module that implements both behaviours and shares the one `gh` runner and the one set of credentials. The callers are moved one module at a time, and the configuration's pair defaults to GitHub for both. The whole existing test suite passes without change to what it asserts, because the stand-in `gh` is now the GitHub implementation's concern. A guard fails the build when `ControlPlane.GitHub`, `gh` or a `repos/` endpoint is referenced outside the GitHub implementation, and is shown failing on a planted violation first (as AGENTS.md asks of a new guard). The split is itself proved at this step, before any second system exists: a test configures a stand-in tracker and a different stand-in code host for one repository and takes a ticket through settling.
3. **Emit events,** with the receiver of ADR 0039, so that polls and webhooks call the same handlers.
4. **Add a second implementation** of a behaviour only when a ticket asks for it, and only after the unverified points above are verified against the system's documentation and a real project. A second implementation may need its own decision: Jira as a tracker needs a mapping of labels, epics and links that ADR 0003 does not give, and GitLab as a code host needs one for approval and checks. The first test of the split with real systems is Jira for tickets and GitLab for changes.

## Consequences

The control plane's logic is written once and each system is a module, so adding a Jira tracker or a GitLab code host is writing one implementation against a set of operations that has been listed from what the code does. A tracker is not written against each code host. A change to GitHub's payloads lands in one place. The webhook receiver emits the same events whichever system sent them.

Two behaviours cost more than one:

- **A link between the systems** that the control plane keeps, and a contract change so that a ticket's key is text. A change opened by hand for a ticket has no link.
- **Two identities and two sets of credentials** for a repository whose systems differ, each to be kept, rotated and kept out of logs, beside the review App's key.
- **The approval authority question.** The recommended mapping is a list the operator maintains, and an unmapped approver does nothing, which is safe and can look like a failure. Anything other than option B replaces ADR 0023's rule for the separate case and needs an ADR of its own.
- **Partial failure.** Settling reads one system and writes the other, so one being down is a pass that cannot finish. The rule already in place carries over: a ticket is settled only once its labels and comment are written, and otherwise tried again. A change can merge while its ticket cannot be closed because the tracker is down, and the closing is then retried and never undone; the control plane has to be able to say, from reading both, which half has happened.
- **A larger behaviour surface** to keep coherent: 28 operations, a normalisation layer that can hide a distinction a system makes (mergeability, checks and a tracker's hierarchy are the likeliest), a data migration for the pair and the ticket key, and a period during which the code is being moved with nothing visible to gain except the guard.

The capability sets cost a feature on a system that lacks it, which is the honest alternative to an operation that quietly does nothing. A tracker that cannot give a label's actor and time cannot be a tracker for approval (ADR 0023), and that is a limit of the design.

**What the receiver of ADR 0039 must do with two sources.** ADR 0039 is written as if one system sent every webhook, and it is not edited here. With a pair, the receiver has to: have one route and one secret for each system, because each signs its own way and a signature is checked by the source's own scheme; key a delivery by the source and its delivery id and not the id alone, because two systems' ids can collide; match a change event to its ticket by the record the control plane keeps (link option C) and a ticket event to its change in the same way, so that an event from either system finds the workload; run its backstop for each system on its own period and let one system's pass fail without stopping the other's, logging a missed delivery for the source that missed it; and take the period per tracker, since a tracker whose webhooks are less reliable than the code host's needs a shorter backstop. The events a webhook of each system maps to are the ones above; the mapping of Jira's and GitLab's hooks, their signing and their retries are the implementations' (to be verified). The accepted ADR 0039's table of polls and the events replacing them is by GitHub's event names and stays correct for GitHub; the tracker rows (intake, epic roll-up, replies, reopened) are what a Jira receiver would replace, and a note or a short ADR on the receiver for two sources is a follow-on, not part of this.

What this does not remove, and how the split changes it:

- **The Rust side.** `heph workload` reads both systems through `gh` (the ticket and its blockers, then the repository and its base), so with a split it needs two inputs or the control plane has to build the workload from what the behaviours return; and the executor's delivery pushes and opens changes with `gh`, which is the code host's, while its description's closing keyword and the ticket number in the branch and the contract are the tracker's and the link's. A ticket key that is text changes the contract. `heph baseline` reads a repository's settings and is code-host-only. None of this is decided here.
- **The Athenaeum source.** The Athenaeum is neither the managed repository's tracker nor its code host. Its skills and lenses are read with the code host's `file` and `directory`, so a configured source for the Athenaeum is a third place to name: a code-host system and a repository. A repository whose code is on GitLab may still read its Athenaeum from GitHub. Which is not decided here.

Not decided here:

- The Jira tracker and the GitLab code host, and everything about either marked to be verified.
- The spelling of the repository's pair in the configuration, and of the approver mapping.
- The receiver for two sources, beyond the requirements above.
- The Rust side and the agents' own read-only `gh` access (the prompts of review, evaluation and questioning agents tell the agent to use `gh`), and the Athenaeum's configured source.
- Whether one repository's tickets may be spread over more than one tracker, which is assumed not.
- How a tracker with no labels as such, or whose workflow has statuses, carries what the labels carry today (ready, running, failed), which depends on what Jira allows and is to be verified.
