# 0041. A gate's last lines are kept in the run record and served behind the login

## Status

Accepted

## Context

A person looking at a ticket whose run failed cannot see why from the dashboard. The reason of a run can carry agent text, and the output of a gate can carry private paths from the machine it ran on, so neither is posted to the code host, where a public repository would publish it. Both are useful to the person who signs in to the dashboard.

Athenaeum's gate result and the executor's `GateFinished` event carry what the gate concluded, not what it printed. Both are shared contracts and must stay exactly as the gate produced them.

## Decision

- **`GateRun` in the run record gains `output` and `omitted_lines`.** `output` is the last 40 lines the gate printed, and `omitted_lines` is how many lines it printed before those. `GateFinished` and Athenaeum's gate result are not changed.
- **The executor fills both for every gate that ran, passed or failed.** It already keeps what a gate prints while it runs.
- **The executor redacts before it sends.** Output can carry private paths and, if a gate echoes one, a secret the executor holds. The record is written with every held secret value taken out, as the event stream is. A private path is not a held secret and stays in the text, which is why the output is served only behind the login and never posted to the code host.
- **The control plane cuts to 40 lines again on read**, so a record from an executor that kept more is still served as 40, and says how many lines were left out in all.
- **Only a signed-in person reads it.** The run's `reason` in full and a run's gates with their output are served by routes that need a session; a request without one is refused with 401.

## Consequences

- The run record grows by at most 40 lines per gate.
- A run recorded before this change has gates with no output; they are served with an empty output and no omitted lines.
- The text is shown as text, never as markup, by whatever displays it.
