# The control plane

The Elixir part of Hephaestium. It keeps the record of every workload: what a
workload was told, and the events the executor said happened, in order, in
Postgres. It will find the tickets to run, start the executor on them and settle
them; the decisions are in [ADR 0022](../docs/adr/0022-the-control-plane-keeps-a-workloads-events-in-postgres-and-enforces-the-stream-rules-itself.md)
and the design in [the walking skeleton](../docs/design/walking-skeleton.md).

## What it has so far

- `ControlPlane.Workloads.create/1` keeps a workload, given as the contract's JSON
  mapping, queued.
- `ControlPlane.Events.append/2` keeps the next event of a workload's stream, and
  refuses one that repeats a sequence, leaves a gap, goes back in time, comes
  after `finished`, or belongs to another workload, with the rule it breaks. The
  state of the workload follows its events and ends with the outcome.
- Events are append-only: the database refuses to change or delete one.
- Postgres cannot keep a NUL byte in text or in jsonb, so each in an event, a run record or a
  failure reason is replaced by U+FFFD (`ControlPlane.Clean`), and nothing raises. An event is not
  refused for it, because that would leave a gap in the stream; the raw line stays in the run's
  own `events.jsonl`.
- `ControlPlane.Runner.enqueue/1` puts a workload on the queue, once, and an Oban job
  (`ControlPlane.Jobs.RunWorkload`) runs it: it starts the executor as a child
  process, keeps every line the executor writes as an event as it comes, and ends
  the workload from how the executor ends. A stream that breaks a rule, or a line
  that is not an event, ends the workload as failed and stops the executor; so does
  an executor that says nothing for too long (`silence_limit_ms`). Oban retries
  starting the executor, never the work inside it. `ControlPlane.Runner.cancel/1`
  tells a running executor to stop, which it answers by ending the workload as
  cancelled.
- `ControlPlane.Intake.run/1` finds the tickets a maintainer has approved and starts
  a workload for each, as [ADR 0023](../docs/adr/0023-intake-reads-an-approval-from-the-ticket-and-fails-closed.md)
  says: the label `heph:ready`, applied by someone with write access, standing only
  while no one without write access has edited the ticket since. Anything GitHub or
  `heph` cannot say is not an approval. An Oban job (`ControlPlane.Jobs.Intake`)
  runs it every minute for each repository in `HEPH_REPOSITORIES`.
- `ControlPlane.Tickets` moves a ticket's labels as its workload progresses: when
  the executor starts, `heph:running` is added and `heph:ready` (the approval, now
  spent) is removed; when it ends, the label that says how is left (`heph:failed` for
  a failure or a timeout, `heph:blocked` for a blocked one, none for a cancelled
  one) and a comment says what happened, with the reason, the question for a
  blocked one, the pull request and what it cost. A completed workload's ticket keeps
  `heph:running` until its pull request is green, which is the next piece. A label
  or comment that cannot be written never changes how a workload ended. The labels
  and comments are written with `gh`, so its login must be able to write issues.
- `ControlPlane.Settle`, run every minute by `ControlPlane.Jobs.SettleTickets`, makes
  done mean green: a ticket whose workload completed is settled only once its pull
  request's required checks are. It reads the pull request, the checks the base
  branch's ruleset requires, and the check runs on the head commit. Every check on the
  commit counts, required or not, and the required ones must have appeared. All green (or
  skipped): `heph:done`, which means the pull request is ready for a person's decision:
  its checks are green and any reviews approved it. It does not mean the ticket's goal is
  verified, and the comment says so, and says when no review was asked for. Any failed, cancelled or timed out: `heph:failed`, naming
  them, without waiting for the rest. Merged: done. Closed unmerged: the running
  label goes and a comment says so. A check still running or not yet seen, no
  checks at all, or GitHub unable to say waits and writes nothing. Only the newest
  run of a check counts, since a cancelled duplicate run leaves a failed run of
  the same name behind. A ticket is marked settled (`settled_at`) only once its
  labels and comment were written. Only the first 100 check runs are read.
- Re-running cancelled checks is off by default. `HEPH_RERUNS=N` lets settle re-run a
  required check whose newest run was cancelled, at most N times per check, before
  calling the ticket failed; the count is kept on the workload (`reruns`). Only
  `cancelled` is re-run: `failure` and `timed_out` are real results, and a real failure
  beside a cancelled check fails at once without spending a re-run. A cancelled run
  that a newer run replaced, or a check the ruleset does not require, is not re-run.
  A re-run GitHub refuses waits and is not counted. The final failure comment says
  the check was re-run.
- Gates left to CI ([ADR 0028](../docs/adr/0028-a-gate-the-workspace-cannot-run-is-left-to-the-ci-check-that-exercises-it.md)).
  When the executor could not run a gate in the workspace and the repository names a
  `ci_check` for it, the run record says so, and settle adds that check to the ones
  that must have appeared and passed on the head commit. It is judged like any check
  (a failure is revised or fails the ticket). If it has not appeared once every other
  check is done and twenty minutes have passed since the workload ended, the ticket
  fails and the comment names the gate and the check, instead of waiting for ever. A
  done ticket's comment lists what was left to CI.
- Revision on red ([ADR 0024](../docs/adr/0024-a-failing-check-is-investigated-and-fixed-or-the-ticket-is-blocked.md))
  is off by default. `HEPH_REVISIONS=N` lets settle, when a pull request's required
  checks really fail (`failure` or `timed_out`, never `cancelled`), start a revision
  instead of failing the ticket, at most N times per pull request. A revision is the
  stored workload again with a new id, `parent_id` set to the workload it revises, its
  base commit moved to the pull request's head, and a `revision` section: the pull
  request, its branch, the attempt, and for each failing check its name, result and an
  excerpt (the check's title and summary and the tail of its job's log, colour codes
  removed, cut to 8,000 bytes). The first workload is marked settled and the revision
  kept and queued in one transaction, so a revision starts once. The ticket's labels do
  not change and a comment says revision k of N has started. A pull request that is not
  on a `heph/` branch of the same repository is never revised. When no revisions are
  left, the ticket is labelled `heph:failed` and the comment says what each revision
  did. When a revision ends, its comment carries the agent's last message, as the
  agent's account of what it found and did (or says it gave none): cleaned of control
  characters, cut at 2,000 characters, and in a fenced block its own text cannot end,
  so a mention or a command in it does nothing. So does the comment of a first run that
  ended without a pull request (it failed, timed out or was cancelled): what its agent
  learned is what the person who approved the ticket needs to act on. A revision that
  changed nothing but said why is still `heph:failed`: the ticket's goal (a change with green checks) was
  not met, and `heph:blocked` is for a workload that stopped to ask a person
  something (ADR 0020). Nothing in the control plane merges a pull request.
- Merging a green pull request is off by default and chosen per repository:
  `HEPH_AUTOMERGE=owner/name,owner/name`. For a listed repository, when every check
  on the head commit is green (see above) and the pull request is open, settle merges
  it, squashed and pinned to the head it judged. It is never a bypass: it merges only
  when GitHub reports the pull request as `clean`, which it does only when the
  repository's own rules are met (a required review, a branch up to date), because an
  account that may bypass those rules would otherwise merge a blocked pull request
  through the API without anyone saying so. A pull request that is blocked, behind,
  conflicting, unstable or a draft, one that is not a same-repository `heph/` branch,
  and one whose merge GitHub refuses are left for a person, and the ticket says why
  (`Not merged: ...`). A state GitHub has not worked out yet, and a merge that failed
  for a reason that may pass, wait for the next pass. It is attempted once per ticket.
- The spend ledger
  ([ADR 0036](../docs/adr/0036-spend-is-kept-as-an-append-only-ledger-by-step-ticket-epic-and-branch.md)).
  When a workload's record is kept, one row for each cost-bearing step is written to
  `spend_entries`: the step (the record's `steps`, or 0 for a workload that lists none), its kind,
  lens and round, the unit, the source and the amount, the parts of a token amount when the
  runtime gave them, and the repository, ticket, epic (the ticket's parent when the workload was
  built), pull request and branch. A unit is kept as it was said and never converted. A record
  kept twice counts once. The table only grows: the database refuses an update, a delete or a
  truncate. A ledger that could not be written does not fail keeping the record; when the control
  plane starts, every workload that has a record and no rows is written (that is also how the
  history before the ledger comes in).
- Reading the ledger
  ([ADR 0036](../docs/adr/0036-spend-is-kept-as-an-append-only-ledger-by-step-ticket-epic-and-branch.md)).
  `ControlPlane.Spend` rolls it up by ticket (its totals, and what each kind of step spent), by
  epic (its tickets, most spent first), by branch and by UTC day, each unit on its own and never
  added to another. `mix spend --ticket N`, `--epic N`, `--branch NAME` or `--days N` prints them
  (`--repo owner/name`, or the one repository configured). A ticket's status comment says what the
  ticket has spent so far over all its workloads and by kind of step, including the run it reports,
  and for a ticket under an epic, what the epic has spent over its tickets.
- Reading a change before it is pushed
  ([ADR 0035](../docs/adr/0035-a-change-is-reviewed-locally-under-every-lens-and-pushed-only-when-each-has-approved-it.md)).
  For a repository whose reviews are asked for (below), a first run's workload and every
  revision's carry every lens of the pinned release that has one, with its text, and
  `prepushRounds`: `HEPH_PREPUSH_ROUNDS` (a whole number, two if it is not set, 0 to read
  nothing). The executor reads its own change under the lenses the change needs and pushes
  only when each that blocks has approved it. The library is read once for a release and kept,
  and one that cannot be read means no workload is built until it can be, as with reviews. A
  review of the pushed commit never carries them, nor anything of what the first round found.
  When a run listed its agents, the ticket's comment says what the implementation, the reading
  and the fixing each spent, units kept apart.
- Asking for reviews is off by default and chosen per repository:
  `HEPH_REVIEWS=owner/name,...`, with `HEPH_ATHENAEUM_REF=v0.17.0` naming the
  Athenaeum release the lenses are read from (an operator's choice, never anything a
  pull request can change; without it reviews are not asked for and the ticket waits).
  For a listed repository, when every check on a pull request's head commit is green,
  settle reads the pull request's changed files and its manifest at that commit,
  asks `heph lenses` which lenses the change needs, and keeps and queues a review
  workload (ADR 0025) for each lens that has none yet at this head. A review is
  asked for once per pull request, head commit and lens; a new head is a new review.
  A lens whose text is not in the release is reported, not skipped silently. If the
  lenses cannot be worked out, the ticket waits. A review leaves the ticket alone.
  What the verdicts at the pull request's head commit say decides what happens next
  (ADR 0026), in this order: a lens that blocks and found something that blocks starts
  a revision told each blocking finding, from the same budget as a failing check
  (`HEPH_REVISIONS`); with none left, or off, the ticket is `heph:failed` and says what
  was found; a lens that could not review (its run did not end well, it left no
  verdict, or its text is not in the release) fails the ticket, and is never an
  approval; a review still running makes the ticket wait; and when every lens that
  blocks approved, the ticket is labelled `heph:done` and says what each lens said, and
  what no lens looked at. A lens whose own
  text says `blocking: false` is advisory: it is waited for and reported and never
  stops the change. A verdict for another head commit never counts. Merging, when
  it is on, happens only after that, and only when GitHub says `clean`.
- Lenses that apply to every change ([#135](https://github.com/erinversfeldcodes/hephaestium/issues/135)).
  A component names one pack, so a lens about the change itself (`spec`: does it do what
  its ticket asked) or about the system it runs in (`system`: a bound moved without a
  measurement) cannot be chosen by a path. A lens whose front matter says
  `applies: always` is asked for on every change that is reviewed, beside the ones the
  diff selected, once each, and is reviewed, deduplicated, assessed and counted like any
  other. The control plane lists the packs of the pinned release (`HEPH_ATHENAEUM_REF`)
  and reads each lens to find the flag, so a listing or a lens it cannot read means
  nothing is asked for until it can be. A change with no changed files asks for nothing,
  and a pack directory with no `review.md` is ignored. The flag counts only in the front
  matter.
- Restart recovery ([#133](https://github.com/erinversfeldcodes/hephaestium/issues/133)).
  When the control plane starts, every workload that was `preparing`, `running` or
  `gating` and last updated before this start is failed, saying the control plane stopped
  and the run was lost, and its ticket is told as for any failed workload (a lost review is
  written on its pull request as "could not review"). A workload updated after the start
  is left alone, and so is a `queued` one, whose job is still in the queue. A lost workload
  is not re-run: re-approve the ticket, or resume its pull request. The jobs in the
  `workloads` queue that an earlier control plane left `executing` are cancelled in the same
  step, since nothing else would end them; a job started since this start, or still waiting,
  is left alone. It is a setting, `recover_on_start`, on unless set false, and off in the
  test suite.
- Resume ([ADR 0030](../docs/adr/0030-re-approving-a-ticket-resumes-its-open-pull-request.md)).
  When a maintainer labels a ticket `heph:ready` again and its pull request is open and a
  `heph/` branch of the repository, intake resumes that pull request instead of starting
  a workload: the workload that completed with it is unsettled again, so settling looks at
  it as it is now; the ticket's state labels become `heph:running` and a comment says who
  resumed it. A resume grants a fresh `HEPH_REVISIONS`, counted from the attempt the pull
  request had reached. An approval resumes once. A merged or closed pull request, one the
  runtime did not make, and one GitHub cannot be asked about are skipped with the reason.
- On the pull request ([#127](https://github.com/erinversfeldcodes/hephaestium/issues/127)).
  When a review ends, one comment on the pull request says which lens read which commit,
  its verdict, and each finding with its severity, whether it blocks, where it is
  (`path:line`), its evidence and what to revise; a review that could not run, or left
  no verdict, says that, and that it is no approval. When a revision ends, its account
  is written there as well as on the ticket. Comments only: nothing is submitted as a
  GitHub review, an approval or a request for changes, because the repository's approval
  is for a person. What an agent wrote is in a fenced block it cannot end, cleaned and
  cut; the verdict word, severity, lens name, commit and pull request number are
  checked before they are repeated. Each is written once (`reported_at` is claimed
  before the write and given back if it fails), and a write that fails is logged and
  never changes how a workload ended.

## Running it

It needs Elixir 1.19 and Postgres. The tests make a database of their own,
`hephaestium_test`, on the local server, and touch nothing else.

```
cd control-plane
mix deps.get
mix test
```

The executor that is run is `HEPH_EXECUTOR` (the one called `executor` on the path
if it is not set), and each run keeps its files, as `heph run` does, under
`HEPH_RUNS_DIR`. A run's checkout (`work/`) is deleted when the run completes, and a run that did
not complete keeps its checkout for `HEPH_WORKSPACE_KEEP_HOURS` hours (72 if it is not set) and
is swept hourly after that; the workload, events, log and record of every run are kept. A run is not
started when the disk the runs are kept on has less than `HEPH_MIN_FREE_MB` megabytes free (10240 if
it is not set; 0 turns the check off): the workload stays queued, a warning in the log says why, and
it is looked at again five minutes later.

Intake takes in the tickets of the repositories in `HEPH_REPOSITORIES` (a comma
separated list of `owner/name`; none if it is not set). It reads GitHub with `gh`
(`HEPH_GH` if it is not the one on the path) and builds workloads with `heph`
(`HEPH_BIN`); neither is given a token by the control plane, and `gh` uses its own
login. A repository named there has its approved tickets run for real, with
delivery.

In the tests, `gh`, `heph` and the executor are refused unless a test stands in for
them with a program made in the temporary directory (`ControlPlane.Programs`),
because a test that ran the real ones would act on a real repository.

`DATABASE_URL` (for example `ecto://user:password@host/database`) says where the
database is when it is not the local default.

`HEPH_PG_SOCKET_DIR` (for example `/tmp`) makes the control plane, and the tests, connect
through the Unix socket in that directory instead of TCP. Set it when another project's
script on the same machine stops every process that holds a TCP connection to Postgres on
port 5432 (thestacks' `scripts/test-elixir.sh` does, and stopped the control plane six
times in a day): a socket connection is not one. Postgres shows it by a connection with no
client address in `pg_stat_activity`.
