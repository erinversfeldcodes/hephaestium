# 0028. A gate the workspace cannot run is left to the CI check that exercises it

## Status

Accepted

## Context

A gate in a repository's manifest is run in the workspace after the agent, and a gate that fails, or cannot be run, ends the workload as failed before any pull request exists (ADR 0014). The first real run on a repository whose CI already runs the same tests, with the database and the secret they need, was failed for lack of that database and secret in the workspace. The agent's money was spent, and the one place that would have judged the change, the pull request's required checks, never saw it. A workspace is a smaller place than CI: it can lack a service, run out of memory or time, or be flaky in ways CI is not. A gate failing there says little about the change.

- Option 1: Leave it as it is. A workspace that cannot run a gate blocks the pull request, and the repository must make its workspace as good as its CI.
- Option 2: Treat a gate that cannot be run as passed or skipped. Nothing says anything else runs it, so a gate no one runs looks like a gate that passed.
- Option 3: Let the repository say which check on a pull request exercises the same work as a gate, and leave a gate to that check only then, report what was left, and make the ticket wait for that check.

## Decision

We will go with Option 3.

- A gate may name `ci_check` in the manifest: the name of the check on a pull request that runs the same work. The repository, not the tool, says they are equivalent.
- The executor leaves a gate to its check only when the gate names one, and only when the gate could not be run where it was: a requirement it declares is unmet, or it timed out. A gate that ran and failed is a failure, whatever check names it, and a gate that names no check is never left to anything. A missing command is not left either, because its exit code cannot be told from the gate's own.
- The result is `deferred`, with the reason and the check. It is never a pass. `heph gate run` does not defer: it is the pipeline's own gate, and a pipeline that deferred to itself would check nothing (ADR 0014).
- The pull request's description lists each gate not run in the workspace, why, and which check covers it.
- The ticket is not done until the check has run and passed on the pull request's head commit. A pull request on which that check never appears fails the ticket, naming the gate and the check; one on which it fails is revised like any failing check.

## Consequences

A workspace that cannot reproduce CI no longer blocks a pull request that CI will judge, and everything left to CI is reported where a reviewer reads it. What stops a mistaken `ci_check` is the control plane's requirement that the check passed, not the executor's trust: a repository that names a check that does not exercise the gate gets a pull request that is settled on a check that proves nothing, and that is a fault in its manifest, visible in the pull request. Reviews, which are separate, are unaffected. Deferral protects a workload from the workspace's limits only; a gate that fails for real still fails, so a flaky gate that fails rather than times out still blocks.
