# The runtime's contracts

The shapes that cross a boundary inside Hephaestium: between the control plane
(Elixir), the executor (Rust), the evaluation code (Python) and the dashboard
(TypeScript). They are Protobuf, and the wire form is the Protobuf JSON mapping,
one message to a line, so a run can be read, replayed and diffed with ordinary
tools. See [ADR-0001](../docs/adr/0001-stack-by-component.md), and the walking
skeleton's design for what each contract is for.

The files, in `hephaestium/v1alpha1/`:

| File | Holds |
|---|---|
| `common.proto` | the cost unit, shared by the others |
| `workload.proto` | `Workload`: what a run is told |
| `event.proto` | `Event`: one thing that happened, one of twelve kinds |
| `run_record.proto` | `RunRecord`: what is kept of a finished workload |
| `adapter.proto` | `Adapter` (what an agent runtime's adapter says about itself, and the table of which runtimes work with which providers) and `LaunchPlan` (how it starts the runtime, with environment variable names and never their values) |

The package is `hephaestium.v1alpha1`. `buf` requires a version suffix, and
`v1alpha1` says plainly that a contract may still change until the skeleton has
used it.

## How a contract is held to account

- `buf lint` holds the files to its standard style, and on a pull request `buf
  breaking` fails a change that breaks what the base branch has.
- [`cases/runtime/`](../cases/runtime) holds, for each message, JSON that must be
  accepted (`valid`) and JSON that must be rejected (`invalid`), each invalid one
  with a sidecar `.violates` stating the rule it breaks as `<kind> <what>`.
- `scripts/check-contract-cases.py` is a strict reader that is graded by the
  cases. It exists because `buf convert` discards unknown fields and unknown enum
  names, so it would accept a misspelt field. A reader in any other language is
  graded by the same cases.

Locally:

```
buf lint
buf build -o /tmp/runtime.binpb
python3 scripts/check-contract-cases.py /tmp/runtime.binpb cases/runtime   # needs the protobuf package
```

## Rules about a stream of events

A single `Event` is checked by the cases. A sequence of them has rules that are
properties of the sequence: it starts at 1 and has no gaps, `finished` is the
last event and appears once, and times do not go backwards. They are enforced
where a stream is read, with their own cases when that exists.

## What the schema does not say

It carries types and names, not rules about values: that a commit is forty
hexadecimal digits, or that a timeout is positive. The readers enforce those,
and a later change can move them into the schema with `protovalidate`.
