# 0015. The baseline check reads the tree and says what it did not check

## Status

Accepted

## Context

The baseline lists what every repository must have. Some items can be answered from a repository's working tree, and some only from GitHub's settings. A first reader has to decide where the items come from, what it does with an item it cannot answer, and how a repository records a deliberate gap. A tool that reports "all clear" while it skipped the questions it could not answer is worse than no tool.

- Option 1: Read the items from the Athenaeum release the repository's lock pins, fetched at run time. The repository and the baseline then agree, and the check needs the network.
- Option 2: Read the items from the Athenaeum release this tool pins as a build dependency, as the other readers do. The tool is graded against the list it checks, and the check works offline.
- Option 3: As Option 2, and refuse to pass any repository when an item could not be checked.

## Decision

We will go with Option 2, and report what could not be checked and not count it either way.

- The items are the ones in the pinned Athenaeum release. A newer baseline arrives with a newer `heph`, as a new Athenaeum tag does for every other contract.
- Each item ends in one outcome: `satisfied`, `unsatisfied` with what is missing or which file and action break the rule, `excepted` with the reason the repository gave, `not_applicable` where a workflow rule is asked of a repository with no workflows, or `not_checked` where the item needs GitHub.
- `not_checked` is never a pass and never a failure. The report counts them, so a reader sees how much was not asked. The check fails, with exit code 1, only when a `required` item is unsatisfied and not excepted. A `recommended` item is reported and never fails the check.
- The answers file is optional for this command. Without it there are no exceptions, and the report says so. If it is there, it must be valid against its schema, and an exception for an item that does not exist is an error and not a silent drop. These end in exit code 3, which ADR 0011 gives a check that could not be made.
- An action is pinned when it names a full commit, a version tag, a path in the repository, or a docker image with a digest. A branch name, no reference, or a tag that is not a version is not.
- Workflows are read with `yaml-rust2`, a YAML 1.2 parser written in safe Rust, because a workflow file is input from a repository the tool does not control.

## Consequences

A repository learns, in one run and offline, which of its files and workflows fall short, and which items nobody asked about yet. The settings and ruleset items stay `not_checked` until a later command reads GitHub. A version-looking tag that has been moved is still counted as pinned, as a tag can move: only a full commit cannot, and a stricter rule can be a new rule in the baseline later. Adding a dependency for YAML is the cost of reading workflows properly, and it is a new thing for the project to keep current.
