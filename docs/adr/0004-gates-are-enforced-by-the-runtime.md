# 0004. A gate is enforced by the runtime, never only by a prompt

## Status

Accepted

## Context

Every rule in the harness taken from The Stacks is an instruction to an agent. Its own text records the results: an audit found 42 of 43 tickets in one campaign had a design review and only 1 had the stack review that review deferred to; waves were reported complete when they were not; a feedback log that every failed review should append to has no entries. Forge's runner, by contrast, checks for itself that a pull request is ready with its required checks green.

- Option 1: Keep the rules as instructions and add audits that catch violations afterwards.
- Option 2: Hephaestium runs each gate itself, selects the required review lenses from the paths a change touches, pins each review to a commit, counts revision cycles, and decides whether work is done from its own records.

## Decision

We will go with Option 2. A gate is a command declared in the manifest that returns a structured result. A tool that is missing fails the gate unless the manifest allows the skip. When the runtime takes over a rule, the matching instruction is deleted from the skill.

## Consequences

Completion is a query over run records, not a statement by an agent. Skills become shorter as rules move out of them. A rule Hephaestium cannot observe cannot be a gate and stays advisory.
