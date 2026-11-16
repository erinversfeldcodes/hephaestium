# 0001. Each component is written in the language that fits its job

## Status

Accepted

## Context

Hephaestium has five kinds of work: supervising long-running workloads, setting up a sandbox and running commands inside it, comparing runs statistically, showing runs to a person, and carrying events between all of these. Forge, the closest prior system, does the first four in TypeScript with systemd units, timers and SQLite.

- Option 1: One language throughout, TypeScript, as Forge does.
- Option 2: One language throughout, Rust.
- Option 3: A language per component: Elixir and OTP with Oban and Postgres for the control plane, Rust for the executor and command-line tool, Python for replay and evaluation, TypeScript and React for the dashboard, and Protobuf for the contracts between them.

## Decision

We will go with Option 3. A workload is a supervised process with a heartbeat, a budget and a timeout, which is what OTP provides. The executor must be one small binary that can be placed inside any workspace. Evaluation leans on Python's statistics libraries. The dashboard is a separate read-only client. Protobuf schemas are generated into all four languages, so no component restates another's types.

## Consequences

Supervision, retries and interrupted-run detection come from the platform. The cost is four toolchains and a code generation step. A change to an event's shape is one schema change. Kubernetes, an external message broker and Terraform are not part of this stack, and adding one is a new decision.
