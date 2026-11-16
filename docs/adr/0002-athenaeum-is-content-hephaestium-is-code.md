# 0002. Athenaeum is content, Hephaestium is code, and a repository describes itself in a manifest

## Status

Accepted

## Context

The harness in The Stacks mixes three kinds of thing in the same files: process that is true anywhere, knowledge that is true for a stack, and facts about one product. The same fact is often stated in several files, and they have drifted apart: whether generated code is committed is stated four ways, and at least 14 cited paths do not exist.

- Option 1: Keep one repository holding skills, tooling and runtime.
- Option 2: Two repositories. Athenaeum holds skills, contracts and packs and no code. Hephaestium holds all code. Each managed repository carries a manifest describing its components, gates, workspace bootstrap, documents and review policy, and a lock pinning an Athenaeum revision.
- Option 3: As Option 2, with the command-line tool in Athenaeum.

## Decision

We will go with Option 2. Process facts have one home in a skill, stack facts in a pack, and product facts in the manifest or the repository's own documents. Athenaeum's own checks verify that every path and skill a file cites exists.

## Consequences

A repository can adopt the skills by hand before any runtime exists. A skill may not name a product module, so some specialist content becomes manifest values. Updates to skills reach a repository as a change to its lock, which can be reviewed and reverted.
