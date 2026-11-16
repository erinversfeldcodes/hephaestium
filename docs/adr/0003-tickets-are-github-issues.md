# 0003. Tickets are GitHub issues

## Status

Accepted

## Context

The Stacks tracks work as markdown files under `issues/` with state in JSON files under `plans/`. Both directories are ignored by git, so they exist in one checkout only, and the hook that checked them finds files through git and now sees none. Local numbers are a different namespace from GitHub's, which the skill that mirrors them warns can close the wrong issue.

- Option 1: Support local markdown tickets and GitHub issues behind a tracker adapter.
- Option 2: GitHub issues only, with native dependencies and sub-issues.

## Decision

We will go with Option 2. A ticket is a GitHub issue. Blocking edges are issue dependencies, an epic is a parent issue with sub-issues, and run state belongs to Hephaestium.

## Consequences

A cited number either resolves or does not. The local issue tools, the mirroring hook and its helpers are retired, not ported. Hephaestium depends on GitHub for its tracker. Tickets in a public repository are public, so a ticket must not hold anything that should stay private.
