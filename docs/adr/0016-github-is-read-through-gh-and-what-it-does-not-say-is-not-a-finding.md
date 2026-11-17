# 0016. GitHub is read through gh, and what it does not say is not a finding

## Status

Accepted

## Context

Some baseline items can only be answered from GitHub: whether secret scanning is on, whether Dependabot alerts are on, what the default branch's ruleset requires. Reading them needs a way to reach GitHub, a decision about whose credentials are used, and a rule for what to do when GitHub will not say. A token the tool holds is a secret to keep and a way to do harm. A refusal read as a finding would tell a repository it is non-compliant on something nobody could see.

- Option 1: A GitHub client in the tool, with a token supplied by environment variable or file.
- Option 2: Run `gh api` with an endpoint and nothing else, so the credentials are the ones `gh` already has and every request is a `GET`.
- Option 3: As Option 2, and treat anything GitHub refuses to say as unsatisfied.

## Decision

We will go with Option 2, and report what GitHub does not say as not checked.

- `heph baseline check --settings` reads GitHub. Without the flag GitHub is never contacted. The repository is `--repo OWNER/NAME`, or the `origin` remote of the root when that is on github.com, as https, scp-style ssh or `ssh://`.
- Every call is `gh api <endpoint>`: no method, no body, no token of the tool's own. A missing `gh`, or an error reading the repository itself, is exit code 3, a check that could not be made.
- A setting is read from what GitHub reports, and an endpoint that answers "not found" is how GitHub says some settings are off: Dependabot alerts, and security updates. An endpoint that refuses, or a field GitHub leaves out because the token cannot see it, makes the item `not_checked` with the reason, and never `unsatisfied`.
- A ruleset item is satisfied by the rules of the active branch rulesets that cover the default branch, added together. A disabled or evaluate-only ruleset, a tag ruleset, one that does not cover the default branch, and classic branch protection do not count. The approval item needs at least one approving review, the checks item needs every check the repository's answers name, and the bypass item needs the bypass actors to be exactly the repository admin role by pull request, or none, as the answers say.
- Items that need the repository's answers, the checks and the bypass, are `not_checked` when there is no answers file.
- An item that applies to public repositories only is `not_applicable` for a private one.

## Consequences

The tool holds no credential, and a check cannot change anything on GitHub. A person must have `gh` installed and signed in, and some settings can be read only with admin access, in which case they are reported as not checked. Code scanning is satisfied by a configured default setup or by any analysis, so a repository that uploads results from its own workflow counts. A repository protected only by classic branch protection fails the ruleset items, which is the intended pressure to move to a ruleset. The responses are tested against recorded shapes, so the tool is also run against real repositories, because a recorded shape can be wrong.
