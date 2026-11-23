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
  blocked one (the agent's own words, quoted as data in a fenced block like its account), the
  pull request and what it cost. A run ends blocked when its agent asked a question, when a
  first run changed nothing and left an account, and when it says the base already fixes the
  ticket and proposes a person close it; the ticket then waits for `heph:ready` again. A completed workload's ticket keeps
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
  the check was re-run. A check that ended `cancelled` because its job reached its time
  limit is not a cancellation: GitHub says so only in the job's annotations ("The job has
  exceeded the maximum execution time"), which settle reads, and the check is then a
  `timed_out` failure that is never re-run (a re-run is cold again and times out again) and is
  revised or fails the ticket like any other, with the end of its log. Annotations that cannot
  be read leave it a cancellation.
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

Tests that run at the same time (two worktrees, two agents) need databases of their own, because
one test truncates tables: `MIX_TEST_PARTITION=2 mix test` uses `hephaestium_test2`, made and
migrated by the `test` alias.

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

## Running it hosted

The control plane is also built as a release in a container, for Fly with Neon as the
database. On a laptop nothing changes: it is still started with `mix run --no-halt`, and every
setting below is off unless its variable is set.

### Build and run the image

The image holds the compiled release and nothing else: no settings, no keys, no credentials,
and no model key or repository code. Everything is read from environment variables when it
starts.

```
# from the repository's root: the image holds the dashboard, which is built from ../dashboard
docker build -f control-plane/Dockerfile -t hephaestium-control-plane .

# bring the database to the current schema, as the platform does before a deploy
docker run --rm -e DATABASE_URL=ecto://user:password@host/database \
  hephaestium-control-plane /app/bin/migrate

# run it, with the health listener on 8080
docker run -e DATABASE_URL=ecto://user:password@host/database -e HEPH_HEALTH_PORT=8080 \
  -p 8080:8080 hephaestium-control-plane
curl -i localhost:8080/health
```

`fly.toml` is the Fly configuration: `release_command` runs `/app/bin/migrate` on a temporary
machine made from the new image before the running machines are replaced, and a non-zero exit
stops the deploy. Deploy to one machine from the repository's root, which is the build context the image needs
(`fly deploy --config control-plane/fly.toml --ha=false`). Creating
the app, the Neon project and the secrets is done by hand once, and is not part of the image.

### Settings

| Variable | Effect |
| --- | --- |
| `DATABASE_URL` | The database. A release will not start without it. |
| `HEPH_MIGRATION_DATABASE_URL` | The database as the migrate command connects to it, if it is not `DATABASE_URL`: Neon's direct connection, because a migration holds an advisory lock for its whole run and the pooler does not keep a session. |
| `HEPH_DB_SSL` | `true` connects over TLS, verifying the server's certificate against the system's roots and its hostname. Neon requires TLS. |
| `HEPH_DB_POOL_SIZE` | The size of the connection pool (10 if not set). |
| `HEPH_OBAN_NOTIFIER` | `pg` or `postgres` (the default); see below. |
| `HEPH_HEALTH_PORT` | Starts the health listener on this port. Off if not set. |
| `HEPH_REVIEW_APP_ID`, `HEPH_REVIEW_APP_KEY` | The review App, with its private key held in the variable itself, for a platform whose secrets are variables. `HEPH_REVIEW_APP_KEY_FILE` (a path) still works and is what the laptop uses; if both are set the variable is used. |
| `HEPH_API_PORT` | Starts the HTTP API (the login and the read-only endpoints) on this port. Off if not set. When it is set the next four are needed, and the control plane does not start without them. |
| `HEPH_API_SECRET_KEY_BASE` | The key sessions are signed with, at least 32 bytes (`openssl rand -base64 48`). `HEPH_API_SECRET_KEY_BASE_FILE` names a file that holds it. Changing it signs everyone out. |
| `HEPH_API_GITHUB_CLIENT_ID`, `HEPH_API_GITHUB_CLIENT_SECRET` | The GitHub App's client id and a client secret, for its user authorization. `HEPH_API_GITHUB_CLIENT_SECRET_FILE` (and `_ID_FILE`) name files that hold them. |
| `HEPH_API_ALLOWED_LOGINS` | The GitHub logins that may sign in, comma-separated, compared without regard to case. Empty lets no one in. |
| `HEPH_EXECUTOR_LINK` | `workers` lets executors on developers' machines run the workloads over the API (see below); `local` or unset keeps the control plane starting an executor itself. `workers` needs `HEPH_API_PORT`. |
| `HEPH_LEASE_SECONDS` | With the link on: how long a claimed workload stays held after the last event or heartbeat (120). |
| `HEPH_CLAIM_WAIT_SECONDS` | With the link on: how long a claim waits for a workload before answering that there is none (25; keep it under the idle time of any proxy in front). |
| `HEPH_API_STATIC_DIR` | A directory holding the built dashboard (`index.html` and its assets), which the API then serves on its own listener. The image sets it. Off if not set: the API then answers exactly as it did without it. The control plane does not start if it is set to a directory with no `index.html`. |
| `HEPH_WEBHOOK_SECRET` | The webhook secret itself, for a platform that keeps secrets as variables (Fly). Turns on `POST /webhooks/github` as the file variable does, is read for each delivery, and is refused together with `HEPH_WEBHOOK_SECRET_FILE`. |
| `HEPH_WEBHOOK_SECRET_FILE` | The path of a file holding the webhook secret, which turns on `POST /webhooks/github` (see below). Off if not set; it needs `HEPH_API_PORT`, and the control plane does not start if the file is empty or cannot be read. The file is read again for each delivery, so the secret can be rotated without a restart. |
| `HEPH_API_BASE_URL` | The address the API is served at, such as `https://heph.example`. The callback is `<base>/auth/callback`. Cookies are marked secure unless this is `http://`. |

`GET /health` answers `200` with `{"status":"ok"}` when the database answers a query, and `503`
with the reason when it does not or does not answer within two seconds. Nothing else is served.
The Claude and GitHub tokens are used only by the executor, which does not run in the container,
so the container is given neither.

### The API and the login

With `HEPH_API_PORT` set, a listener of its own serves a login and read-only endpoints. Everything
except the three login routes is behind a session, and any other request is `401`, a path that
does not exist included. Only `GET` and `HEAD` are answered: every other method is `405`, because
nothing here writes, except the six `POST` routes of the executor link below, which exist only
while the link is on, and the webhook route, which exists only while webhooks are on.

| Route | What it gives |
| --- | --- |
| `GET /auth/login`, `/auth/callback`, `/auth/logout` | The login (GitHub App user authorization), and ending the session. Logout is a `GET`, so it can be sent cross-site; all it can do is sign the person out. |
| `GET /api/session` | The login of the session. |
| `GET /api/tickets` | The tickets with workloads: title, how many workloads, the latest and its state (`limit`, `offset`). |
| `GET /api/tickets/:owner/:name/:number` | A ticket's workloads and what it spent. |
| `GET /api/workloads/:id`, `/events`, `/steps` | A workload; a page of its events (`after`, `limit`); the steps its record lists. |
| `GET /api/workloads/:id/events/stream` | The events after `after` (or `Last-Event-ID`), then each one as it is kept, as server-sent events, until the event that finishes the workload. |
| `GET /api/spend/tickets/...`, `/epics/...`, `/branch`, `/days` | The spend roll-ups. |
| `GET /api/questions` | Workloads that ended blocked with a question, for tickets with no later workload. |

With `HEPH_API_STATIC_DIR` set, any `GET` or `HEAD` outside `/api`, `/auth` and `/worker` is answered
with the file it names, or with `index.html` when it names none and has no file extension (so a reload
of a page of the dashboard works), or `404` when it has an extension and no file. These files need no
session; they hold no data. A path that could leave the directory is `404`.

A person signs in with GitHub, and the login must be on `HEPH_API_ALLOWED_LOGINS`; the allowlist
decides who may open the API and gives no authority over a repository. The session is a signed
cookie (secure, http-only, same-site `Lax`, eight hours) holding the login and nothing else, and is
checked against the allowlist on every request. A login state is good for one callback and ten
minutes.

Wiring the GitHub App for the first time (the live flow has not been exercised against github.com):
set the App's callback URL to exactly `<HEPH_API_BASE_URL>/auth/callback`; the flow asks for no
OAuth scopes. The user token the login is given is kept, sealed with a key made from
`HEPH_API_SECRET_KEY_BASE`, in a cookie of its own that lives as long as the session, because the
dashboard's reply to an agent's question is made with it, as the person; nothing else reads it and
signing out drops it. The App therefore needs Issues write for that reply (a user token can do no
more than the App and the person may). The token is not refreshed: with "Expire user authorization
tokens" on, a reply made after it has lapsed is a `401` that asks the person to sign in again. The
full list of the App's settings is in [the runbook](../docs/runbook-hosted-control-plane.md). Check
that a login that is on the list reaches `GET /api/session` and one that is not gets `403`.

### The executor link

With `HEPH_EXECUTOR_LINK=workers` the control plane no longer starts an executor: a workload is
left `queued` until the executor of the person who approved its ticket asks for it. Off, none of
these routes exists and the API is as it was. The executor opens every connection (a long poll
and plain `POST`s, so nothing listens on the developer's machine and any proxy carries it). It
holds a device token and nothing else.
The executor's side is `heph worker --control-plane URL` (see the repository README), which takes the
token from `HEPH_WORKER_TOKEN`, `HEPH_WORKER_TOKEN_FILE` or `~/.config/hephaestium/worker-token`.

| Route | Who | What it does |
| --- | --- | --- |
| `GET /api/csrf` | session | The CSRF token the two writes below need. |
| `GET /api/device-tokens` | session | The person's own tokens: name, when made, last seen, revoked. |
| `POST /api/device-tokens` | session + CSRF | `{"name": "laptop"}`: makes a token. It is in the answer once and is kept only as a hash. |
| `POST /api/device-tokens/:id/revoke` | session + CSRF | Revokes one of the person's tokens; what it held lapses at once. |
| `POST /worker/claim` | bearer token | Waits for a workload the token's login approved. `200` with `workload`, `lease.seconds` and `nextSequence`, or `204`. |
| `POST /worker/workloads/:id/events` | bearer token | `{"events": [...]}`, kept in order by the stream's rules; each moves the lease on. A rule broken is `422` and the workload is failed. A batch sent again is accepted without being kept twice. |
| `POST /worker/workloads/:id/heartbeat` | bearer token | Moves the lease on. |
| `POST /worker/workloads/:id/record` | bearer token | The run record, at the end. Kept as the local runner keeps it, and the lease is let go. |

The two kinds of write never stand in for each other: a worker route ignores a session cookie
and a browser route ignores a bearer token. A request about a workload the token does not hold
(it lapsed, was let go, or is another person's) is `409`.

A claim is a lease that ends `HEPH_LEASE_SECONDS` after the last event or heartbeat, checked by
the clock on every request. A workload whose lease lapses is put back on the queue only if the
stream has no `deliveryBegan` event (nothing could have been pushed) and it has been put back
fewer than twice; otherwise it is failed, as a lost run is locally. The events the lost run said
stay kept (events are append-only) and the next claim says where the numbering goes on
(`nextSequence`). A workload whose approver's executor is not connected waits, for as long as it
takes.

### Webhooks

With `HEPH_WEBHOOK_SECRET_FILE` set, the API also serves `POST /webhooks/github`, and the control
plane learns what happened from GitHub's deliveries instead of asking every minute. Off, the route
does not exist and the control plane polls as before.

A delivery is accepted only if `X-Hub-Signature-256` is the HMAC-SHA256 of the exact body under the
secret (compared in constant time); anything else is `401` before the body is read, stored or
enqueued. A good one is answered `202` after one transaction that keeps its `X-GitHub-Delivery` id
and one job; a delivery id seen before is `200` and nothing more, and ids are forgotten after a
week. The job holds only the control plane's own event (kind, repository, number, actor, time,
delivery id) and never GitHub's payload. A delivery for a repository not in `HEPH_REPOSITORIES`, or
of a kind not subscribed to, is acknowledged and dropped.

Subscribe the App to `issues`, `issue_comment`, `pull_request`, `pull_request_review`,
`check_run` and `push`. A delivery is a hint to look: the handler reads the current state through
the tracker and the code host and calls what the poll calls, so a late or repeated delivery does
nothing and a forged approval is no approval.

With webhooks on, the six jobs that read the code host (intake, settling, keeping pull requests
current, judging merges, rolling up epics, answering replies) run once an hour, each at its own
minute, and once when the control plane starts, as the backstop for a delivery that was dropped.
A pass that finds work a delivery should have woken logs `missed delivery of <kind>`. A workload
that ends settles its pull request at once, because no webhook says that it ended.

### Oban's notifier and Neon's pooler

Neon's pooled connection is pgbouncer in transaction mode: a server connection is lent for one
transaction and then taken back, so a session never keeps a `LISTEN`. Oban's default notifier
is `LISTEN`/`NOTIFY`, so through the pooler it would connect, appear to work and never be told
anything.

The hosted configuration sets `HEPH_OBAN_NOTIFIER=pg`, which is `Oban.Notifiers.PG`: the
notifier uses Erlang's process groups and sends nothing through the database. It was chosen over
a direct connection for the notifier because Oban's Postgres notifier listens on the connection
settings of the Repo, so a second, direct URL for it alone is not possible without every query
also using the direct connection, which gives up the pooler (Neon's direct connections are
limited in number, and the pooler is what lets a small compute serve many clients). Anything the
notifier carries is a hint that makes Oban act sooner; without it queues still poll, so a lost
notification costs latency, not a job.

The reason it is correct here and the condition on it: `pg` only reaches the nodes of one Erlang
cluster. The control plane runs as one machine, where that is every node. A second machine that
is not clustered would not be told what the first did, and would only poll. If the control plane
ever runs on more than one machine, either cluster them or use `HEPH_OBAN_NOTIFIER=postgres`
with `DATABASE_URL` set to the direct connection.

Not shown: this has not been run against a pooled Neon connection, because none was available.
What is shown is that the setting selects the notifier (`hosted_config_test.exs`).
