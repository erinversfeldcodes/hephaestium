# 0007. The default branch takes changes only by pull request, with one approval that the maintainer may bypass

## Status

Accepted

## Context

Hephaestium will give workloads a token that can write to a managed repository, and it will manage itself. Forge refuses to dispatch to a repository unless its default branch requires an approving review, approval after the last push, and no bypass for Forge's own App. GitHub does not let an author approve their own pull request, and today the only author is the one maintainer.

- Option 1: Pull request and checks required, no approval required yet.
- Option 2: One approval required, with the repository administrator able to bypass it when merging a pull request.
- Option 3: One approval required and no bypass.

## Decision

We will go with Option 2. A ruleset on the default branch requires a pull request with one approval, approval of the last push, resolved review threads, the `ci` check, linear history and squash merges, and forbids deletion and force pushes. The administrator role may bypass these only by merging a pull request, never by pushing. Actions may not approve pull requests.

## Consequences

Every change is a pull request with a green `ci` check. While the maintainer authors the pull requests, the approval is bypassed on every merge, so it protects nothing yet. Once Hephaestium's own GitHub App authors pull requests, the maintainer's approval is a real second pair of eyes, and the bypass should be removed in a new decision.
