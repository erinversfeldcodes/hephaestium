# 0008. A version is derived at release, stamped into the artefacts, and never committed

## Status

Accepted

## Context

A managed repository pins the Athenaeum revision it uses, and Hephaestium will ship a command-line tool and later a control plane. Both need versions that increment without anyone remembering to bump them, and every released artefact must say which version it is. In `martian-robots-tests` the release workflow writes the version into every file that states it and pushes a `chore(release)` commit to the default branch. That works there because the branch is unprotected. Here the default branch takes changes only by pull request (ADR-0007), so a workflow cannot push to it.

- Option 1: A release commit pushed by the workflow, with a release identity allowed past the branch rule.
- Option 2: A release pull request that bumps the version files, tagged and published when a maintainer merges it.
- Option 3: No version in the tree. After the checks pass on the default branch, a workflow reads the commits since the last `v<version>` tag: a breaking change bumps the major version, `feat` the minor, `fix` and `perf` the patch, and anything else releases nothing. It builds the artefacts with that version compiled in, creates the tag, and publishes a GitHub release with the artefacts and notes.

## Decision

We will go with Option 3, in both repositories, using the same helper script.

- The workflow exports the version as `HEPHAESTIUM_VERSION` before any build step. A component reads it at build time and falls back to a development version when it is absent.
- Each build step proves that its binary reports the version being released before packaging it into `dist/`. Everything in `dist/` is attached to the release.
- The ticket that adds a buildable component adds its build step to the release workflow. A component that builds in CI and not in the release workflow is not done.
- Every commit that can land on the default branch must be a conventional commit, so the pull request title and each of its commits are checked as part of the required `ci` check.
- Once the Protobuf contracts and the manifest schema exist, a breaking change detected mechanically against the last tag makes the release a major one whatever the commit types say. The repository owns that instrument, as `martian-robots-tests` owns its probe.
- Release tags may not be moved or deleted.

## Consequences

Merging a `feat` or `fix` releases it, with no release step to forget and no exception to the branch rule. The changelog is the list of GitHub releases, not a file. A released binary reports its version; a build from an untagged checkout reports a development version, and a file in the tree never states a release number that could go stale. Installing from source at a tag does not get the stamped version unless the build is given it. A commit with the wrong type releases the wrong version or none, which is why the type is checked before merge. Until there is something to build, a release carries notes and no artefacts. Athenaeum has nothing to build: its artefact is the tagged tree.
