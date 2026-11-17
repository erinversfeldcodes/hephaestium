# 0019. The tree a check reads is the repository, not its directory

## Status

Accepted. It refines how ADR 0015's check finds the files it reads.

## Context

The baseline check and apply read a working tree: the files that satisfy file items, the workflows, and the manifests that show which ecosystems a repository uses. A working tree holds more than the repository. On the maintainer's own checkout of gazebolang it held an agent worktree, a second copy of the repository that git excludes, and `heph baseline apply` read its manifests and wrote Dependabot entries for them. Anything git ignores, and any other checkout, is not the repository.

- Option 1: Keep walking the directory and skip a list of names (`.git`, `target`, `node_modules`) and any directory with its own `.git`.
- Option 2: Ask git what the repository contains, and walk only when git cannot say.
- Option 3: Parse `.gitignore` files ourselves.

## Decision

We will go with Option 2.

- Where the root is inside a git repository and git can be run, the files are the ones `git ls-files --cached --others --exclude-standard` lists: what is tracked and what is new and not ignored. A tracked file that was deleted from the disk does not count, a directory git lists whole is another repository and does not count, and the list is only what is under the root even when the root is a subdirectory.
- Under either listing, nothing under a directory that has a `.git` of its own, as a directory or as a file, is the repository's. Git does not always recognise a nested checkout as one.
- Where git is not there, or the root is not a repository, the directory is walked as before: it does not enter `.git`, `target`, `node_modules` or a directory with its own `.git`, and does not follow a link.

## Consequences

The check reads what a clone of the repository would hold, so a CI checkout and a developer's crowded one agree. A file that is ignored does not satisfy an item, which is right: it would not exist for anyone else. A file that is new and not yet added counts, so a change in progress can be checked before it is committed. Option 3 would reimplement git's rules, badly. The cost is running `git` for each check, which takes a few milliseconds, and a dependence on it being installed that the fallback covers.
