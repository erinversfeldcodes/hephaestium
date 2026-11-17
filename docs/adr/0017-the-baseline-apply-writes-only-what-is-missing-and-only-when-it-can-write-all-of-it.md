# 0017. The baseline apply writes only what is missing, and only when it can write all of it

## Status

Accepted

## Context

The baseline pack holds templates for the files a repository needs. A tool that writes them into a working tree can do harm in three ways: overwrite something a person wrote, write outside the repository through a link, or leave the tree half done when it fails part way. The files are for a pull request, so the working tree is the right place to write them, and git makes a mistake there recoverable, but only if nothing was lost first.

- Option 1: Overwrite with `--force`, and otherwise refuse when any destination exists.
- Option 2: Write only what does not exist, report what was skipped, and never overwrite.
- Option 3: As Option 2, and write each file as soon as it is worked out.

## Decision

We will go with Option 2, and work everything out before writing anything.

- An item is applied only when the baseline check finds it unsatisfied and the repository has not excepted it, and only if it names templates. Satisfied, excepted and template-less items write nothing.
- Every file is worked out first: the template is found, each `{{variable}}` is filled in, and the destination is examined. A value the repository's answers do not give, a licence with no templates, a template path with a variable the tool does not know, a missing answers file or an unnamed repository is exit code 3, and no file is touched.
- A destination that exists, whether a file, a directory or a link, is skipped and reported. A path whose parent is a symbolic link or not a directory is refused outright. A file is created exclusively, so it cannot replace anything even if the tree changes between the check and the write.
- The values come from the repository's answers and from its name, taken from `--repo` or the github.com `origin` remote, and the year from the clock in UTC.
- After writing, the baseline check runs again, and the result is a gate result named `baseline-apply` that lists what was written, what was skipped and why, and what is still unsatisfied. It exits 1 if a required item remains, which is the case for any item with no template. `--dry-run` writes nothing and reports what it would.
- Settings on GitHub are not changed by this command. That is a separate command, and it will need an explicit flag.

## Consequences

Running it twice writes nothing the second time. A repository that already has a file keeps it, even if it is a worse one than the template, and the tool says so and leaves the decision to the person. The generated files are ordinary files in a working tree, reviewed and committed like any other. A write that fails part way, because the disk is full, leaves the files written so far and says which. The exit code stays 1 until every required item is satisfied, so a first run on a repository that also lacks a `dependabot.yml` or has unpinned actions does not look finished, and that is deliberate.
