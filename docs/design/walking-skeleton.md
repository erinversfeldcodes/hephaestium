# The walking skeleton

Status: a proposal, for the maintainer to review before any of it is built.

The aim is one thing working from end to end before any part is made complete:
a maintainer approves a ticket on a managed repository, Hephaestium runs one
agent on it in a workspace, runs the repository's gates itself, and opens a
pull request, keeping a record of everything on the way. Gazebolang is the first
managed repository. The decisions in ADR 0020 (a person stops a workload only to
approve its ticket) and ADR 0021 (an epic is an integration branch) apply, and
the skeleton handles a single ticket and leaves epics for later.

## What is deliberately not in it

Review lenses and the steward, the red-first check, test custody, revision
cycles, epics, the dashboard, the replay harness, and agent runtimes other than
Claude Code. Each is a later piece, and the skeleton is shaped so they can be
added without changing the parts it builds.

## The parts

### Control plane (Elixir, OTP, Oban, Postgres)

- **Intake.** Finds tickets: issues on the managed repositories it is told about,
  approved by a maintainer. It reads GitHub by running `gh`, as `heph` does
  (ADR 0016), and holds no token.
- **Workloads.** One supervised process for each workload, a state machine:
  `queued`, `preparing`, `running`, `gating`, then one of `completed`, `failed`,
  `blocked`, `timed_out` or `cancelled`. A heartbeat from the executor keeps it
  alive, and a missed one ends it as `failed`. Oban holds the queue and retries
  the dispatching of a workload, never the work inside it.
- **Record.** An append-only table of events, and tables for tickets, workloads
  and gate results, all in Postgres. The record is the only source of "is it
  done". Nothing reads an agent's own claim of completion.

### Executor (Rust, a binary beside `heph`)

- Takes one workload specification, creates the workspace (a clone at the base
  commit), puts the goal documents in it read-only, runs the repository's
  bootstrap command, starts the agent runtime through its adapter, and relays
  the adapter's events as it goes.
- Enforces the workload's timeout and budget, and stops everything it started,
  as `heph gate run` already does.
- Runs the gates by calling `heph gate run`, and relays each result.
- Pushes the workload's branch and opens the pull request with `gh`, if the
  gates passed.
- The control plane starts it as a child process and talks to it in lines of
  JSON on standard input and output, in the shapes the contracts below define.
  Starting it on another machine is a later change to the transport, not to the
  messages.

### Adapter (one, for Claude Code)

An adapter says how to start an agent runtime and how to turn what it prints
into the event contract. The first one starts Claude Code in non-interactive
mode with structured output. Cost is recorded in the unit and from the source
the runtime reports, never converted: a subscription is not a price per token
(ADR 0005).

## The contracts, in Protobuf

They live in this repository under `proto/`, checked in CI with `buf` for style
and for breaking changes against the last release. The wire form is the Protobuf
JSON mapping, one message to a line, so a run can be read, replayed and
diffed with ordinary tools.

| Contract | What it carries |
|---|---|
| `Workload` | What a run is told: the ticket, the repository and base commit, the manifest and lock it ran under, the goal documents, the agent runtime and provider, the timeout and the budget |
| `Event` | One thing that happened, with its sequence number and time: started, an agent message, a tool call and its result, a skill loaded, a gate started and its result, cost, a heartbeat, finished |
| `RunRecord` | What the control plane keeps of a finished workload: its final state, the gate results, the cost by unit, the configuration, and the hash of each skill it loaded |
| `Adapter` | What an adapter must provide: how to start the runtime, which events it can produce, and which providers it works with |

A gate result is not redefined here: an event carries Athenaeum's gate result
as it is, so the two never disagree.

## What this needs that does not exist

- **A bootstrap command in the manifest.** The context document says a manifest
  holds how to bootstrap a workspace, but version 0 of the manifest has no such
  key. The executor needs it, so it is a new key in Athenaeum, and in `heph`.
- **A way to say a ticket is approved.** A label a maintainer applies is the
  simplest, and the executor refuses a workload whose approval was not given by
  someone with write access. The label's name is for the maintainer to choose.
- **Somewhere safe to run an agent.** The first executor isolates a workload
  with a directory of its own, a process group and a trimmed environment. That
  is not a security boundary: an agent running as the maintainer can still do
  what the maintainer can. Containers or virtual machines are a later piece, and
  until then the skeleton runs only against repositories and tickets the
  maintainer trusts.

## The order it would be built in

1. The contracts, and the manifest's bootstrap key.
2. The executor, run by hand with a workload file, against a stub adapter that
   plays a recorded transcript, so everything but the real agent is tested.
3. The Claude Code adapter, against recorded output first and then a real run.
4. The control plane, with intake, the state machine and the record.
5. The first real ticket, on gazebolang, end to end.

Each is its own ticket, with tests first, and each leaves the repository
working.

## Decisions the maintainer made

1. **Isolation.** A directory, a process group and a trimmed environment are
   enough for the first version, which runs only the maintainer's own trusted
   tickets. A container comes before the runtime runs unattended or reads
   anything the maintainer did not write, and the workload gets a narrow token of
   its own and never the maintainer's login.
2. **Approval.** A label applied by a maintainer, with the label as the only way
   to approve and no approval by comment. The labels are `heph:ready`,
   `heph:running`, `heph:done`, `heph:failed` and `heph:blocked`. As in Forge, an
   approval stands only while no one without write access has edited the ticket
   after it. The rule is built with the control plane's intake.
3. **The pull request.** The executor opens it, after the gates pass, so the
   agent never holds a token that can push. Watching the pull request's required
   checks, and treating a ticket as done only once they are green, is a later
   piece of the runtime.
