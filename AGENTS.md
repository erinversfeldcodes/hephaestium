# AGENTS.md

Rules for any agent, and any person, changing this repository.

## Decisions

- When making a technical decision, do not give much weight to development
  cost. Prefer quality, simplicity, robustness and long-term maintainability.
- Add a technology only where the project needs it. The test: would a
  principled engineer build this here even if no one were watching?
- A decision that would be costly to reverse is recorded in `docs/adr/` before
  it is built. ADRs are read-only once accepted; a change of mind is a new ADR
  that supersedes the old one.
- Use the vocabulary in `docs/CONTEXT.md`, including its "avoid" terms.

## Scope

- One concern per ticket and per pull request. A second concern found on the
  way becomes its own ticket.
- Once a ticket's scope is approved it is frozen. Bug fixes inside that scope
  are the only exception.

## Evidence

- Report with evidence, never assertion. A claim that something works cites
  how it was proven: a named test, a command and its output, or an observed
  result.
- Reading code does not prove behaviour. If you did not run it, say so.
- A new test is shown failing for the right reason before the code that
  satisfies it is written.
- A check that runs on synthetic data proves the artefact is well formed, not
  that it works. Say which kind a check is.
- A new gate or guard is shown failing on a planted violation before it is
  trusted.
- A requirement about keyboard use, a colour theme or layout at a width is
  written "a browser test shows ...", so that it is settled on a named test.
  jsdom has no layout, focus or colour scheme and cannot show it.

## Code

- State the reason in the code, not a pointer to it. Never cite a ticket
  number, ADR number or plan name in code or comments; write the sentence that
  explains why.
- One home per fact. A value, a rule or a list that exists in two places will
  drift; generate one from the other or link to it.
- Fix lint errors, test failures and flaky tests when you see them, even if
  your change did not cause them. Never dismiss a failure as unrelated without
  reproducing or explaining it.

## Writing

- Commits follow Conventional Commits. The subject is a lower-case sentence
  that says what changed and why it matters, not which files moved.
- Never add agent attribution of any kind to a commit message, a pull request
  title or body, an issue or a comment. That covers `Co-Authored-By` trailers,
  "Generated with" credits, and session links.
- Never commit a secret, a credential, or anything copied from a private or
  client system.

## Done

- A change is not done until the required checks on its pull request are
  green.
- Never push to `main`. Every change arrives by pull request.
