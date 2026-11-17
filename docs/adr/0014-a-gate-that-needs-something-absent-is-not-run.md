# 0014. A gate that needs something absent is not run

## Status

Accepted

## Context

A gate may need environment variables the repository cannot ship: a database address, an API key. When one is absent, the gate's own command fails somewhere inside the test run, or skips the part that needed it, and a reader takes that for a failure of the code under test. A caller needs to tell "the code is wrong" from "the gate could not run".

- Option 1: Let the gate's command fail on its own and leave the explanation to its error message. No new rule, and the confusion stays.
- Option 2: Report a gate with an absent prerequisite as `skipped`. It exits 0, so a pipeline that lacks a secret goes green without having checked anything.
- Option 3: Treat it as a gate that could not be run, with the exit code that ADR 0011 already gives that case, and say which names are absent.

## Decision

We will go with Option 3.

- A gate's `requires` table names the variables it needs, as `env` or as `secret`. Before running a gate that applies to the change, `heph gate run` looks each name up in its own environment. A name that is unset or empty is absent.
- If any are absent, nothing is run, standard output stays empty, and standard error names each absent variable and whether it is a secret. The exit code is 3. A value is never printed, so a secret cannot leak through the report.
- A gate that does not apply to the change, because of `when_paths`, is skipped as before and needs nothing.
- `heph` reads its own environment only. It does not read a `.env` file, so a variable that lives only in one is absent to it, and a person exports it first. Whether `heph` should read such a file is a separate decision.
- The gate result contract does not change: a gate that could not be run has no result, as in every other case of exit code 3.

## Consequences

A missing prerequisite can never be a pass, a skip or a failure of the code. A repository that keeps its secrets in a file its own task runner loads must export them before running a gate through `heph`, which is a small cost for local runs and none in a pipeline, where the secret store supplies the environment. Services that must be reachable, rather than variables that must be set, are not covered and can be added as a separate optional key.
