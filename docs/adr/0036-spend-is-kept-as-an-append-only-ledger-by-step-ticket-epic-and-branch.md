# 0036. Spend is kept as an append-only ledger by step, ticket, epic and branch

## Status

Accepted

## Context

What a run spent is kept, but only as a total on each workload's record, and nothing ties the totals together. What is wrong with that, as it stands:

- **The token figure hides its parts.** The Claude Code adapter adds input, cache-creation, cache-read and output tokens into one number before it reports (`COST_UNIT_TOKENS`), and the dollar figure is the runtime's own total. What a run cost is not told apart from how: cache reads dominate a long run and cost far less per token than output, so the single figure says little about where spend went, and it cannot be compared across providers that bill the parts differently.
- **Nothing sums spend across the work that belongs together.** A ticket's spend is spread over its first run, each revision, every review of the pushed commit, its completeness evaluations and its gap steps, each a workload with its own record. A ticket that is an epic's child has no spend that rolls up into the epic, though the tracker records the parent (`Ticket.parent`, `sub_issues`) and the executor records the branch (`PullRequestOpened.head_branch`, `Revision.branch`).
- **Steps inside a run are now listed** (ADR 0035: the implementation, each reading, each revision), so what a ticket spent on reading can be told from what it spent on the work, but only inside one workload's record, in a field nothing aggregates.
- **Evidence (2026-11-22, this machine's database).** 256 workloads across 13 tickets since 2026-11-18, 49 of them reviews. The median completed run cost $0.146 (192 runs, largest $0.471). So spend is small per run, and there is no way to say what an epic cost, what reviewing cost against implementing, or what a branch cost end to end, without reading each record by hand.
- **What is deliberately absent:** a spend cap. There is none per ticket, per day or per repository, and none is wanted yet. When one is, it will be per unit: dollars for a provider that bills them (OpenRouter), tokens for one that meters them (Cortex), and the two are never converted into each other.

The options:

- Option 1: Leave it. Read each workload's record when a number is wanted.
- Option 2: An append-only ledger in the control plane's database, one row per step of every workload that spent, written when the record is kept, with roll-ups by ticket, epic, branch and day computed from it; and the token figure reported with its parts.
- Option 3: Report spend to an external system (OpenTelemetry metrics, a billing store) and aggregate there. It needs a service this project does not run yet, and the relationships (epic, branch) are the control plane's to supply.

## Decision

We will go with Option 2.

- **Tokens are reported with their parts.** A `Cost` of unit tokens may carry a breakdown of counts by category (input, output, cache creation, cache read). The total stays what it is; the breakdown is beside it, never instead of it, and a runtime that reports no parts reports none. Dollars and tokens are still never converted into each other, and a figure the runtime reported is still told apart from one Hephaestium estimated.
- **A ledger, written once.** When the control plane keeps a workload's record, it appends one row per cost-bearing step: the workload, the step (its kind, lens and round, or the one step of a run that lists none), the unit, the source, the amount and the token parts if any, with the ticket, the epic (the ticket's parent when the workload was built), the pull request and the branch it concerned, and when. Rows are never updated or deleted. A record kept twice does not count twice: the row is identified by workload and step.
- **Roll-ups are queries over the ledger, not stored totals.** Per ticket (with what the implementation, the reading and the fixing each spent), per epic (the sum over its tickets, with each ticket listed beside it), per branch, and per day, each unit on its own. A ticket that is an epic's child counts toward the epic at the time of the spend, so an epic's total does not change when a child is moved.
- **Workloads that predate the ledger are backfilled** from their stored records, as one step each with the totals they have, marked as having no parts. The ledger then has the whole history.
- **Where it shows.** The ticket's status comment already says what a run spent and, since ADR 0035, what each kind of agent spent; the epic's roll-up pass adds what its tickets have spent so far, unit by unit. A fuller view (a dashboard, an API) reads the same ledger and is not decided here.
- **No cap yet, and the ledger is what a cap would read.** A limit per unit, per ticket or per day is a later ADR, configured per repository, and enforced where a budget already is: the workload's own, in the executor.

## Consequences

An epic's cost, the share of spend that went on reading against implementing, and the cost of a branch from first run to merge become a query, and a spend limit has the number it needs to read when one is wanted. The price is a contract change (the breakdown on a token cost, which is additive and optional, so an old executor still works), a table that grows by a few rows per workload, and a backfill. The breakdown applies from when it ships: earlier runs have totals only, and say so. The ledger counts what a runtime reported; a runtime that reports nothing is estimated by Hephaestium and marked as such, so a sum can mix the two sources and says which in each row. Spend inside a ticket's judgement and its gap steps lands on the ticket like any other workload; what is not yet attributed is work that belongs to no ticket (none exists today).

Not decided here: a spend limit and where it applies, a dashboard or API over the ledger, retention of old rows, and any conversion between units, which is never done.
