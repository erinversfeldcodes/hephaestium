# Security

## Reporting a vulnerability

Please report a vulnerability privately, through
[GitHub's private vulnerability reporting](https://github.com/erinversfeldcodes/hephaestium/security/advisories/new)
on this repository's Security tab. Do not open a public issue for it.

Your report is visible only to you and the maintainer.

## Scope

Hephaestium runs coding agents with credentials inside a sandbox, so reports
are especially welcome for:

- A way for a workload to read a secret it was not given, or to reach another
  workload's files or processes
- A way for a workload's token to put code on a default branch without a
  person's approval
- A way to bypass a gate, a required review or test custody
- A way for content in a ticket, a comment or a repository to direct the
  runtime itself

## Response

The maintainer will acknowledge a report within 5 business days and aim to give
a fix or mitigation timeline within 30 days for a confirmed vulnerability. Once
a fix is out, the advisory is published with credit to you.

## Supported versions

The project is at the design stage. Only the latest commit on `main` is
supported.
