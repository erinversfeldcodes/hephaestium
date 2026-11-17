# 0018. Changing GitHub is an explicit act, and never modifies what exists

## Status

Accepted. It adds to ADR 0016, which made every call to GitHub a read: the reads are unchanged, and this is the one command that writes.

## Context

The baseline asks for settings and a default-branch ruleset that live in GitHub and not in git. A tool that can apply them changes something outside the working tree, which a pull request cannot review and git cannot revert. A wrong ruleset can lock the maintainer out of their own default branch, and a wrong setting can open a repository to something it was closed to.

- Option 1: Apply by default, with a flag to preview.
- Option 2: Preview by default, apply only with an explicit flag, and never modify what exists.
- Option 3: Do not apply at all, and print the changes for a person to make.

## Decision

We will go with Option 2.

- `heph baseline apply --settings` reads GitHub exactly as `check --settings` does, works out every request, and prints them with their bodies. It makes none unless `--yes` is given. `--yes` means nothing without `--settings`, and `--settings` does not write files.
- Each request is `gh api -X METHOD ENDPOINT`, with the body on standard input and the credentials `gh` already has. The tool holds no token. Every request goes to the one named repository.
- A setting is changed only if its item is unsatisfied and not excepted. Code scanning and the description are not applied: one needs a choice of languages and the other text only the maintainer can write.
- A ruleset is created only where no active ruleset covers the default branch, with every rule the ruleset items ask for, the checks and the bypass the repository's answers name, and the administrator bypass only by pull request. Where a ruleset already covers the branch, nothing is changed, and what it lacks is reported. An existing ruleset, or classic branch protection, is never modified or deleted.
- A request GitHub refuses is reported with GitHub's message and does not stop the others. The command does not read back what it did, because GitHub's answers may lag: a person runs `heph baseline check --settings` afterwards.
- The answers file is required, because the ruleset needs its checks and its bypass. Without it, without `gh`, or with a repository that cannot be read, it exits 3 and changes nothing.

## Consequences

Running it by mistake changes nothing. A person sees the exact requests before any is made, and can make them by hand instead. A repository that already has a ruleset, even a weaker one, is left as it is, so applying the baseline cannot weaken or replace a decision, and cannot bring a repository with a partial ruleset up to the baseline without a person editing it. Creating a ruleset next to classic branch protection leaves both in force, and removing the older one is the person's decision. The tool has not been shown against a real repository's write path by its author: that is the first use, with the dry run first.
