# 0012. A component's lint commands run as gates named for the component, with a default timeout

## Status

Accepted

## Context

A repository's lint and format checks are declared per component in the manifest, as commands that change nothing and fail on a finding. In The Stacks three hooks repeat the same skeleton, one per language, and each stops at the first failure. `heph lint` replaces them. The manifest gives a gate a timeout and a component's commands none, and the gate result schema (ADR-0004, ADR-0011) describes what a gate reports.

- Option 1: A new result shape for lint, with findings. Richer, but it needs every linter's output understood, which is a different and larger job.
- Option 2: Treat each lint command as a gate, so that it reports with the gate result contract and runs with the same limits and cleanup. Name each `lint:<component>:<n>`, with n counting the component's commands from 1.
- Option 3: As Option 2, and add a timeout to the manifest's component commands now.

## Decision

We will go with Option 2.

- Each command runs from its component's `root`, under the toolchain wrapper, and ends as a gate result on one line of standard output, in order.
- Only components whose `paths` match a file named with `--changed` are linted. Without `--changed`, every component is.
- Every command runs even when an earlier one fails, so one run reports every finding. The exit code is the worst: 0 if all passed, 1 if any failed or timed out, 3 if linting could not be run, which includes a component whose `root` is not a directory. Every root is checked before any command runs.
- A component's command has no timeout in the manifest, so each gets 900 seconds, which `--timeout-seconds` overrides. A timeout in the manifest is a schema change and belongs in Athenaeum, so it is not made here.

## Consequences

Lint reuses everything a gate already does: the wrapper, the process group, the cleanup and the result. A repository's hooks reduce to one command. The result's `gate` field is not a name the manifest declares, so a reader cannot assume every result's `gate` is a key of `gates`. The 900-second default is a guess that suits the slowest lint in The Stacks and may suit nothing else; the right fix is to let the manifest say. Findings are not parsed, so a caller learns that a component failed and not what the linter found; the linter's output is on standard error.
