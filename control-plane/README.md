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
  branch's ruleset requires, and the check runs on the head commit. All green (or
  skipped): `heph:done`. Any failed, cancelled or timed out: `heph:failed`, naming
  them, without waiting for the rest. Merged: done. Closed unmerged: the running
  label goes and a comment says so. A check still running or not yet seen, no
  checks at all, or GitHub unable to say waits and writes nothing. Only the newest
  run of a check counts, since a cancelled duplicate run leaves a failed run of
  the same name behind. A ticket is marked settled (`settled_at`) only once its
  labels and comment were written. Only the first 100 check runs are read.

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
`HEPH_RUNS_DIR`.

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
