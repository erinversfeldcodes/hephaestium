# 0034. A failing preview's own logs are read by the control plane and given to the revision as data

## Status

Proposed

## Context

A revision for a failing check is told what the check's log said (ADR 0024). For a check that runs against a deployed preview of the pull request, that log shows only what the browser saw: a timeout, a page that did not change, a status code. The server's side (the error behind an HTTP 500, an application that was killed, the memory it had) is in the preview's own logs, and Fly keeps only the last 100 lines per app, for a machine that stops itself a few minutes after the run. A revision that starts a few minutes after the failure can still read what is left, but it has to be given the means to.

On gazebolang's preview this was the whole problem: a failing `invite` spec showed a 500 on `POST /api/create_invite`, the server's error was no longer in the buffer, and the revision said it could not tell what had failed.

What it takes to read those logs is a Fly credential. A Fly token that only reads is, as far as is known, scoped to the organisation, not to one app, so it can read every app in it, including production's logs. Whether it is narrower than that has to be confirmed before this is accepted.

- Option 1: Give the revision's agent a read-only Fly token and the `fly` commands to read logs and status, and let it look when the check's log is not enough. The agent decides what to read, but it runs the repository's code and its own changes to it in the same environment, so what it is given, anything it is persuaded to do with it can do, including reading production's logs.
- Option 2: The control plane reads the preview's logs and the machine's status itself, when a failing check belongs to a preview, and gives the text to the revision as data, as it does for a check's log. The agent never holds a credential. It cannot ask for more than was read.
- Option 3: Ship the preview's logs to a store (a log shipper) that an agent may query read-only. It keeps more than 100 lines, and needs a new service and credentials.

## Decision

We will go with Option 2.

- The repository says in its own settings how a pull request's preview app is named (`gazebo-pr-<number>`), and which check is the one that runs against it. Without both, nothing is read.
- When a pull request's failing check is that one, the control plane reads, once, the app's machine status (its state and its events, including any exit and whether it was killed for memory) and the last lines of its log, through `fly` with the read-only token. It reads only that app: the name is built from the pull request's number and the repository's template, and nothing else is accepted.
- The text is cleaned and cut as everything an agent wrote or a system printed is (`ControlPlane.Account`), put in a fenced block its own text cannot end, and handed to the revision with the failing check's log, labelled as the preview's own and as data to read, never as instructions.
- The token is a file the operator names (`HEPH_FLY_READ_TOKEN_FILE`), read when it is needed, passed to `fly` through the environment for that one command and never in an argument, a log or the database. It is not given to the executor, an agent or a gate.
- A log that cannot be read is said so in the revision's instructions, and the revision goes ahead: it is evidence to add, not a gate.
- An agent that needs more than it was given says so in its account, and a person decides whether to widen what is read. A way for an agent to ask for more is a later decision.

## Consequences

A revision for a preview failure starts with the server's side as well as the browser's, so it can tell a memory kill from an application error. The agent never holds a Fly credential, so a persuaded agent cannot read what it was not handed. The control plane now holds a Fly token, which is read-only and could read other apps in the organisation if what is known about its scope is right: its file is as sensitive as the review App's key, and the app name is fixed by the pull request's number, not by anything an agent wrote. What is read is bounded by Fly's own buffer, so a quiet run's last 100 lines are the most the revision sees, and a noisy one may have lost the failure already (the preview's memory sampler must not fill it; that is its own change). A revision cannot choose to look at something else. If that proves too narrow, Option 3 is the way to keep more.
