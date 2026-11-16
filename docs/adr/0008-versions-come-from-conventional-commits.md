# 0008. A version is derived from conventional commits, and a release is a tag

## Status

Accepted

## Context

A managed repository pins the Athenaeum revision it uses, and Hephaestium will ship a command-line tool and later a control plane. Both need versions a person can reason about. A version kept in a file has to be bumped by hand or by a bot commit, and can disagree with the tags.

- Option 1: A version file bumped in a release pull request, with a generated changelog file.
- Option 2: No version file. After the checks pass on the default branch, a workflow reads the commits since the last `v<version>` tag: a breaking change bumps the major version, `feat` the minor, `fix` and `perf` the patch, and anything else releases nothing. It creates the tag and a GitHub release whose notes list those commits.

## Decision

We will go with Option 2, in both repositories, using the same helper script. Every commit that can land on the default branch must be a conventional commit, so the pull request title and each of its commits are checked as part of the required `ci` check. Release tags may not be moved or deleted.

## Consequences

Merging a `feat` or `fix` releases it; there is no separate release step to forget. The changelog is the list of GitHub releases, not a file. A commit with the wrong type releases the wrong version or none, which is why the type is checked before merge. A release is cut only from a commit whose checks passed. Until there is something to build, a release carries notes and no artefacts.
