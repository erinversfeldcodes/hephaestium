# 0039. The control plane learns from webhooks and polls only as a backstop

## Status

Accepted

## Context

The control plane learns what happened on a code host by asking. `control-plane/config/config.exs` has eight cron entries, and each is a job in `control-plane/lib/control_plane/jobs/`:

| Job | Schedule | What it reads, from the code |
| --- | --- | --- |
| `Intake` | every minute | The approved tickets of each repository, then for each: its history, the labeller's permission, its edit history (`Intake.run`, `consider`) |
| `SettleTickets` | every minute | For each completed workload with a pull request not yet settled: the pull request, the base's required checks, the head's check runs, the reviews (`Settle.run`) |
| `KeepCurrent` | every 10 minutes | For each pull request settled in the last 14 days: the pull request, and whether its base moved (`Settle.stale`) |
| `EvaluateMerged` | every 10 minutes | For each ticket settled in the last 14 days and not yet judged: its pull request, to see whether it merged (`Evaluations.merged`) |
| `RollUpEpics` | every 10 minutes | All open tickets of a repository, for those whose sub-tickets are all closed (`Evaluations.epics`) |
| `AnswerReplies` | every 5 minutes | For each ticket whose last step waits on a person: its comments; for each ticket closed on evidence: its events (`Gaps.replies`, `Gaps.reopened`), and `Gaps.unquestioned`, which reads only the database |
| `RecheckOwed` | hourly (minute 7) | The database, and nothing on the code host itself (`Evaluations.recheck`) |
| `SweepWorkspaces` | hourly (minute 17) | The runs directory on disk (`Workspaces.sweep`) |

What that costs, measured on this machine's development database on 2026-11-22 and derived from the code:

- **Four days of polling is most of the table that grew most.** In the four days since 2026-11-18 the database holds 10,498 rows of `oban_jobs`, which is its largest table at 5.9 MB of 25 MB. 4,616 of them are `Intake` and 4,616 are `SettleTickets`, 9,232 together, 88 percent. No Oban pruner is configured (the plugin list in `config.exs` is the cron plugin only), so these rows are never removed.
- **Calls to the code host, derived from the cron lines.** With one repository and nothing in flight, `Intake` makes at least one call a minute (the list of approved tickets), which is at least 60 an hour, and `RollUpEpics` one call every ten minutes, 6 an hour. Each ticket whose pull request is settled and open adds `KeepCurrent` and `EvaluateMerged` reads, 12 an hour, and each ticket waiting on a reply adds 12 an hour (`AnswerReplies`). `SettleTickets` reads the database first and calls the code host only for a workload awaiting settlement. These are arithmetic over the schedules, not counts of real requests.
- **The rate limit is per installation,** on the owner's account of it; the exact figure for an App installation is to be verified against GitHub's documentation.
- **A compute that scales to zero is kept awake by a poll.** The owner's account is that Neon suspends an idle compute and that a poll every minute prevents it. Neon's idle timeout is to be verified. A second source of database traffic that polling does not explain is Oban's own housekeeping (a peer-leadership table, `oban_peers`, exists, and Oban's producers and plugins query the database on their own intervals; to be verified). So fewer jobs is certain, and a compute that stays asleep between events is not, until it is measured on the hosting.
- **Latency.** A ticket approved just after a pass waits up to a minute to start. A pull request that merges is noticed within ten minutes, an epic closed within ten, a reply within five.

What a code host offers instead is a webhook: it sends an HTTP request when something happens. The facts about GitHub's that this design leans on are from memory and are to be verified against its documentation when the receiver is built: a delivery carries its event name, a unique delivery id and a signature of the body (HMAC-SHA256 with a secret) in headers; the sender expects an answer within seconds; and a failed delivery is not retried by itself, though it can be redelivered. That last one is why a backstop is needed. The control plane has no HTTP surface today (ADR 0037 gives it one).

The options:

- Option 1: Keep polling and make it slower, say every ten minutes. Nothing new to build, no new public route. A ticket is noticed up to ten minutes late and every pass still costs calls and database writes, so the saving is a tenth and the latency is ten times worse.
- Option 2: Webhooks only. The control plane is quiet and prompt, and a delivery that is dropped (the control plane was restarting, the code host failed, the receiver rejected it) is a ticket that nothing ever looks at again. This system's rule is that unknown is never green and that it fails closed; a design in which silence cannot be told from nothing happening breaks it.
- Option 3: Webhooks wake the control plane to look, and a slow poll remains as a backstop for what was dropped.

## Decision

We will go with Option 3.

**The receiver.**

- It is one route, authenticated only by signature, and kept apart from the dashboard's routes (ADR 0037). It reads the raw body, computes the HMAC-SHA256 of it with the webhook secret, and compares it with the signature header in constant time. A delivery that does not verify is answered 401, and nothing about it is stored or enqueued.
- It answers at once, after one insert, and does nothing else in the request. A body-size limit lower than the code host's maximum is set.
- **A redelivery is one job.** The delivery id is inserted into a table with a unique index, in the same transaction as the job that handles it, and a conflict is a duplicate, answered success and dropped. Oban's own uniqueness is not enough by itself, because it covers only the jobs still in the table. Delivery ids are kept for a bounded time (recommended: a week) and then deleted.
- **Only a normalised event is kept.** The job's arguments are the provider-neutral event of ADR 0038 (kind, host, repository, number, actor, time, delivery id) and never the code host's payload, which can hold repository text. A delivery of a kind not listed below, or for a repository that is not configured, is acknowledged and dropped without a read of the code host.
- **A delivery is a hint to look, never the thing looked at.** The handler reads the current state through the host behaviour and acts on that. It follows that a late, repeated or out-of-order delivery changes nothing, that a forged claim of an approval is not an approval (ADR 0023 reads it from the ticket's history, and this does not change), and that the existing once-only guards (a workload is started once for each approval time, a report is claimed once) stay what makes a repeat harmless. A handler that finds nothing to do does nothing.
- **A delivery is first matched against the database,** before any call to the code host: only a pull request that belongs to a workload, only a ticket the control plane has met or an approval label, and only a push to the default branch of a repository that has an open settled pull request, can cause a read. Everything else is dropped for free.

**Each poll, and what replaces it.**

- **`Intake` (every minute)** is replaced by `issues` labeled with the ready label (`TicketApproved`): the handler runs `Intake.consider` for that one ticket. `issues` edited and unlabeled (`TicketEdited`) re-check a ticket that is waiting, which matters in hosting because a workload can wait for an executor (ADR 0037); a running workload is untouched, as now. A label other than the ready label is dropped.
- **`SettleTickets` (every minute)** is replaced by four triggers that find the workload by pull request number, or by the head commit when the payload does not carry one:
  - `check_run` completed (`ChecksCompleted`), which is what settling waits for. The settling rule reads check runs only, never commit statuses, so a `status` event is not needed.
  - `pull_request` closed, merged, synchronize and edited (`ChangeClosed`, `ChangeMerged`, `ChangeUpdated`): a closing or a merge is a verdict, and the head moving makes what was read stale.
  - `pull_request_review` submitted or dismissed (`ChangeUpdated`), because the mergeable state changes with a review.
  - **Two that no webhook sends,** because the control plane causes them: a workload completing and a review workload finishing. The job that ends the workload enqueues settling for it, so that these do not wait for a backstop either.
- **`KeepCurrent` (every 10 minutes)** is replaced by `push` to the default branch (`BaseMoved`): the handler finds the open settled pull requests of that repository from the database and runs `Settle.stale` for each. A push to any other branch is dropped, because the pull request's own branch arrives as `synchronize`.
- **`EvaluateMerged` (every 10 minutes)** is replaced by `pull_request` closed and merged (`ChangeMerged`), which calls the same judgement after a merge that settling already does. Judgement is made once for a ticket, so a merge that both settling and this event see is one judgement.
- **`RollUpEpics` (every 10 minutes)** is replaced by `issues` closed (`TicketClosed`) on a ticket: the handler reads the ticket's parent through the host and, if there is one, runs the epic's roll-up for it. It does not rely on a field of the payload to name the parent, because that is not established here (to be verified).
- **`AnswerReplies` (every 5 minutes)** is replaced in two of its three parts: `issue_comment` created (`CommentAdded`) on a ticket whose last step waits on a person, which runs `Gaps.replies` for that ticket (a comment on a pull request is dropped), and `issues` reopened (`TicketReopened`), which runs `Gaps.reopened` for that ticket. The third part, `Gaps.unquestioned`, reads only the database: it finds a ticket whose judgement did not start a questioning, for which no event exists, and it stays on the backstop.
- **`RecheckOwed` (hourly)** is replaced by nothing. It reads no code host. It starts a judgement again once a day (configurable) has passed since the last one owed something, so the trigger is a clock. It stays as it is. Scheduling each recheck at the time it falls due, instead of looking hourly, would let the database sleep between them, and is possible and not decided here.
- **`SweepWorkspaces` (hourly)** is replaced by nothing, and is not a poll of anything remote: it deletes old checkouts from a disk. In a hosted control plane the checkouts are on the executors (ADR 0037), so the sweep leaves the control plane and is not part of this decision.

Webhook events subscribed: `issues`, `issue_comment`, `pull_request`, `pull_request_review`, `check_run` and `push`. `installation` and `installation_repositories` events matter only when an App is installable by other accounts, which is not the case now (ADR 0037 has one person per hosting), and are not subscribed.

**The backstop.** Every job above that reads the code host runs once an hour, offset from the others: `Intake`, `SettleTickets`, `KeepCurrent`, `EvaluateMerged`, `RollUpEpics` and `AnswerReplies`. They are the same functions the handlers call, run over everything instead of one ticket or one pull request. A full pass also runs once when the control plane starts, because a deployment restart is when deliveries are lost, and the code host will not send them again by itself. A pass that finds work a delivery should have woken is logged as a missed delivery, with its kind, so that the backstop's catch rate can be seen and an hour is changed on evidence. Changes to a ruleset or to a repository's required checks send no event listed here, and the backstop is what finds them.

**Other hosts.** A receiver is written for each host and emits the events of ADR 0038. GitLab's Issue, Merge Request, Pipeline, Note and Push hooks are the owner's list of what maps to the same events, and the mapping, and how GitLab signs a delivery (a secret token in a header, to be verified), are for the implementation of that host.

## Consequences

An approval starts a workload within seconds, a check that finishes settles its pull request at once, a merge is judged at once, and a quiet control plane makes no calls to the code host and writes no job rows between events. The hourly backstop replaces 60 intake passes and 60 settling passes an hour with one of each, which is where the 88 percent of the job rows come from, and it leaves the other four jobs at an hour where they were five or ten minutes. Each handler reads one ticket or one pull request, not all of them.

What it costs: a public route, a secret to keep (the webhook secret, beside the App's private key), a table of delivery ids, a mapping from every event to a handler that has to be kept right when GitHub changes what it sends, and a second way for each thing to be noticed, so a bug in a handler is hidden for an hour by the backstop and found only through the missed-delivery log. A reply to a question, a ticket's reopening or an edit that voids an approval can be an hour late when a delivery is dropped, and the backstop is the only answer to that. A ticket approved while the control plane was down is found only by the pass at start-up.

What is not claimed: that the database sleeps. Fewer jobs and no polling make that possible, and Oban's own housekeeping may still keep a compute awake; the hosting has to be measured, and Oban's pruner, which is not configured now, has to be set when the jobs are fewer, because the rows it would remove are 88 percent of the table that grew most.

Not decided here:

- The backstop's period: an hour is a recommendation, changed on the missed-delivery log.
- Whether to ask the code host for its recent deliveries and redeliver the missed ones on start, instead of a full pass.
- How a delivery for a pull request from a fork, or without a pull request number, is matched to a workload, beyond the head commit and the backstop.
- Scheduling a recheck at its due time instead of looking hourly.
- Retention of delivery ids, recommended as a week.
- The receiver's route, body limit and the secret's rotation.
- GitLab's hooks and signing, and the receiver for them.
