# 0013. Sync vendors the pinned skills and never overwrites what it did not create

## Status

Accepted

## Context

A managed repository takes skills from Athenaeum. Where those skills live, how they reach an agent runtime, and what happens when the pin moves are decisions every repository inherits, and a wrong one is expensive: a skill that silently changed under a repository changes how its agents behave. Forge keeps its skills in a directory synced by a scheduled workflow and links them for the runtime. The Stacks already holds its own real copies of several skills in `.claude/skills`, which a sync must not destroy.

- Option 1: Fetch the skills at run time and keep nothing in the repository. Nothing to review, and nothing that works offline.
- Option 2: Vendor the skills the lock names into the repository, and link them for each agent runtime the lock names. Fetch with `git`.
- Option 3: As Option 2, with the vendored copies hashed in the lock, so that drift can be checked without the network.

## Decision

We will go with Option 2.

- The skills a lock names are copied into `.athenaeum/skills/<name>/`, committed, so a change to a skill is a diff a person reviews and a workload needs no network to read its instructions. For `claude-code` each skill is also linked at `.claude/skills/<name>` by a relative symbolic link.
- The source is fetched with `git` at the lock's tag, and the commit the tag resolves to must equal the commit in the lock. A tag that has moved is refused and nothing is changed.
- Sync never overwrites a file or directory it did not create. Everything is compared before anything is written, so a refusal leaves the repository as it was. A vendored skill the lock no longer names is removed, with its link, but only a link into the vendored skills is ever removed.
- `--check` fetches the pin and compares, and changes nothing. It reports each difference as missing, modified, extra, not linked, stale or blocked, and exits 1 if there is one.
- `--update [TAG]` moves the pin to the named release, or to the latest by version number and not by name, rewrites `rev` and `commit` keeping the rest of the lock as written, and then syncs. The lock is written only after the sync succeeds.
- The exit code is 0 when the repository matches, 1 when `--check` finds a difference, 2 for a command line that cannot be understood, and 3 when it could not be synced: the lock is missing or invalid, `git` failed, the pin moved, a skill is not in the release, or something would be overwritten.

## Consequences

A repository works offline, and a skill's history is the repository's own. Adopting an Athenaeum skill in a repository that has its own copy is one deliberate step: remove the local copy, and then sync. Checking for drift needs the network, because the lock holds no hashes of the vendored copies; Option 3 would remove that need at the cost of a larger lock and a second thing to keep in step, and can be added if a check that must run offline appears. Sync needs `git` installed. A skill may contain only files: a symbolic link inside one is refused. The latest release is chosen by the source's tags, so an Athenaeum release is immediately the default for anyone who runs `--update`.
