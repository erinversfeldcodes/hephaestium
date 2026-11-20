# Context

Shared language for Hephaestium and Athenaeum. Use these words, with these
meanings, in code, documents, tickets and reviews.

## Language

**Hephaestium**:
The runtime: the code that reads tickets, creates workspaces, runs workloads,
runs gates, fans out reviews and keeps the record.

**Athenaeum**:
The content the runtime works from: skills, contracts and packs. It holds no
code. A managed repository pins one revision of it.

**Harness**:
Everything around a model that makes it do reliable work: the skills and
contracts in Athenaeum together with the enforcement in Hephaestium.
_Avoid_: using "harness" for `pi`, Claude Code or Cortex Code. Those are agent
runtimes.

**Agent runtime**:
The program that runs the model loop inside a workload: `pi`, Claude Code or
Cortex Code. Hephaestium reaches each through an adapter that normalises its
events to one schema.
_Avoid_: harness, agent CLI

**Provider**:
Where an agent runtime's model is served from, and how its use is paid for:
OpenRouter billed per generation, an Anthropic subscription, or a Snowflake
account. Which runtimes work with which providers is an explicit table, not an
assumption.
_Avoid_: backend, vendor

**Managed repository**:
A repository Hephaestium runs workloads against. It carries a manifest and a
lock.
_Avoid_: target repo, client repo

**Manifest**:
The file in which a managed repository describes itself: its components, its
gates, how to bootstrap a workspace, where its documents live, and its review
policy. Product facts live here or in the repository's own documents, never in
Athenaeum.

**Lock**:
The file in a managed repository that pins the Athenaeum revision it uses.

**Component**:
One buildable part of a managed repository, with a root, the paths that belong
to it, its pack, and its test and lint commands.

**Skill**:
One instruction file that teaches an agent one method. Small enough that a run
either loaded it or did not.
_Avoid_: prompt, role file

**Pack**:
The content that is true for any repository on one stack, such as Elixir or
Postgres, or for one cross-cutting concern, such as privacy.

**Contract**:
The fixed shape of something that crosses a boundary: a specialist's
completion report, a reviewer's verdict, a finding, a gate result.

**Ticket**:
A unit of buildable work, held as a GitHub issue with dependencies.
_Avoid_: issue file, task

**Epic**:
A parent ticket whose child tickets are delivered together on one integration
branch. A ticket is an epic when the tracker records sub-issues under it. A
person, not Hephaestium, judges an epic complete.
_Avoid_: campaign, wave

**Workload**:
One isolated run of an agent runtime against one ticket or one review, created
and torn down as a unit.
_Avoid_: job, session

**Workspace**:
The checkout a workload works in, created by Hephaestium and bootstrapped by
the command the manifest names. Goal documents are mounted into it read-only.
_Avoid_: worktree, sandbox

**Run record**:
What Hephaestium keeps about a workload: its transcript, the skills it loaded,
its gate results, its cost, and the configuration it ran under.

**Gate**:
A command the manifest declares, run by Hephaestium, that returns a structured
result. A gate either passed, failed, or was skipped for a reason the manifest
allows.
_Avoid_: check (for the GitHub status), hook

**Red-first check**:
The gate that runs a ticket's new tests before any implementation exists and
requires them to fail on an assertion, not on a compile error.

**Test custody**:
The rule that the tests for a ticket are written by one workload and mounted
read-only into the workload that implements it.

**Lens**:
One reviewer's point of view, with its own checks, applied to a diff at a
pinned commit: a stack lens, or the contract, privacy, system, test-truth,
design or UX lens. A lens declares whether its verdict blocks.
_Avoid_: reviewer agent

**Verdict**:
What a lens returns: `APPROVED`, `NEEDS_REVISION` or `FAILED`, with findings.

**Finding**:
One thing a lens reports, carrying its lens, a severity, whether it blocks, and
its evidence.

**Steward**:
The persona that judges design: its values, its evidence standard and its
severity registers. Its methods are separate skills.
_Avoid_: staff engineer

**Evidence**:
What backs a claim: a named test, a command and its output, an observed result,
or a link to a run record. A regular expression matching a line of text is not
evidence.

**Replay corpus**:
A frozen set of past tickets, each pinned to its base commit with the tests
that proved it, replayed to compare two configurations on the same work.

**Maintainer**:
A person with write access to a managed repository, and so the only kind of
person Hephaestium takes direction from.
