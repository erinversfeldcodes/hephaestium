# 0022. The control plane keeps a workload's events in Postgres and enforces the stream rules itself

## Status

Accepted

## Context

ADR 0004 says the runtime, not the agent, decides whether work is done, and that the record is the only source of that. The control plane is the part that keeps the record: for each workload, what the executor said it did, in order. The executor writes its events as lines of JSON on its standard output (the walking-skeleton design), and the contract says what a stream must look like: a sequence that starts at 1 and has no gaps, times that never go backwards, one `finished` event and it is last.

The control plane reads that stream from another process. It could trust that the executor wrote a valid one, or check.

- Option 1: Store whatever arrives, and let a reader work out whether the stream is whole.
- Option 2: Store an event only if it is the next in its workload's stream, refuse one that breaks a rule and say which, and keep the events as the contract's own JSON.
- Option 3: Keep events in files and the state in memory, with Postgres as a later index.

## Decision

We will go with Option 2.

- Postgres is the record. A workload is a row; its events are rows, unique by workload and sequence, so that two writers of one sequence cannot both succeed, and never updated or deleted.
- An event is appended only if its sequence is the next one, its time is not before the last, and nothing follows `finished`. Otherwise it is refused with the rule it breaks, and the caller decides what that means for the workload (a stream that breaks the rules is a failed workload, not a repaired one).
- An event is kept as the JSON the contract defines, whole, with its kind and time lifted into columns to be queried. The contract stays the one definition of what an event is, and nothing here restates its fields.
- A workload's state is derived from its events as they arrive, and ends with the outcome `finished` carries.
- The executor stays a child process whose output is lines of that JSON, as already decided; the transport can change later without changing what is kept.

## Consequences

A reader of the record can trust it without checking it again, and a broken stream is found where it breaks, with its cause, instead of in a report later. The cost is that the control plane enforces a contract that the executor also enforces, so a rule changes in two places; the contract's cases are the thing that keeps them agreeing, and are run against both. Keeping events whole as JSON means a query across a field that is not a column reads inside the JSON.
