# 0021. An epic is an integration branch with a pull request for each child

## Status

Accepted

## Context

An epic is a parent ticket whose children are delivered together. Forge delivers each ticket as its own pull request to the default branch. The harness Hephaestium grew out of delivers an epic's children onto one integration branch and opens one pull request for the whole epic, and records why: a child that is green on its own can break the whole.

- Option 1: One pull request per child, each to the default branch. Each lands as soon as it is green, and the default branch can hold half an epic.
- Option 2: An integration branch for the epic, a pull request for each child into it, and one pull request for the epic into the default branch.

## Decision

We will go with Option 2.

- An epic has an integration branch. Each child ticket is delivered by a pull request into it, from a branch the workload makes from the integration branch's current head.
- The whole-repository gates run on the integration branch after every child is merged into it, because a child that passed alone can break the whole.
- When the children are done, one pull request takes the integration branch to the default branch, and a maintainer reviews and merges it, as ADR 0020 says of every pull request to a default branch.
- A ticket with no epic is a child of nothing: one pull request, straight to the default branch.

## Consequences

The default branch only ever receives a complete epic, and a half-finished one lives on a branch that can be abandoned. The cost is a long-lived branch that can drift from the default branch, and a final pull request that is large and reviews a sequence of changes at once. Who merges a child's pull request into the integration branch, and how a conflict with the default branch is handled, are not decided here.
