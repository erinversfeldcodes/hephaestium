import type { Needs, TicketSummary } from "./api";

export type GroupId = "needs" | "flight" | "shipped";

/** The groups, by what they ask of a person. */
export const groups: readonly { readonly id: GroupId; readonly title: string }[] = [
  { id: "needs", title: "Needs you" },
  { id: "flight", title: "In flight" },
  { id: "shipped", title: "Shipped this week" },
];

const week = 7 * 24 * 3_600_000;

function time(at: string | null | undefined): number {
  const parsed = at === null || at === undefined ? Number.NaN : Date.parse(at);
  return Number.isNaN(parsed) ? 0 : parsed;
}

/** When a ticket last changed: its latest run, its settling, or its approval, whichever is last. 0 when none is known. */
export function changedAt(ticket: TicketSummary): number {
  return Math.max(time(ticket.latest?.at), time(ticket.settledAt), time(ticket.waiting?.since));
}

/**
 * The group a ticket belongs in, or null when it belongs on no part of the board: a ticket
 * that was settled or closed more than a week ago has shipped, but not this week. A merged
 * ticket that is not yet judged stays however old it is, as it still has something to come.
 */
export function groupOf(ticket: TicketSummary, now: Date): GroupId | null {
  if (ticket.needs !== null) return "needs";
  const closed = ticket.settledAt !== null || ticket.pullRequest?.state === "closed";
  if (closed) return now.getTime() - changedAt(ticket) <= week ? "shipped" : null;
  if (ticket.pullRequest?.state === "merged") return "shipped";
  return "flight";
}

/** The tickets of each group that has any, the most recently changed first. */
export function grouped(
  tickets: readonly TicketSummary[],
  now: Date,
): readonly { readonly id: GroupId; readonly title: string; readonly tickets: readonly TicketSummary[] }[] {
  return groups
    .map((group) => ({
      ...group,
      tickets: tickets.filter((t) => groupOf(t, now) === group.id).sort((a, b) => changedAt(b) - changedAt(a)),
    }))
    .filter((group) => group.tickets.length > 0);
}

/** How long ago, as a person says it: "40 min ago", "2 h ago", "yesterday". */
export function ago(at: number, now: Date): string {
  if (at === 0) return "at an unknown time";
  const minutes = Math.max(0, Math.floor((now.getTime() - at) / 60_000));
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  return days === 1 ? "yesterday" : `${days} days ago`;
}

/** A short account of a ticket's last event: "failed 2 h ago after 3 runs". */
export function lastEvent(ticket: TicketSummary, now: Date): string {
  const when = ago(changedAt(ticket), now);
  const needs = ticket.needs;
  if (needs?.kind === "failure") return `failed ${when} after ${needs.runs} ${needs.runs === 1 ? "run" : "runs"}`;
  if (needs?.kind === "question") return `asked ${when}`;
  if (ticket.settledAt !== null) return `closed ${when}`;
  if (ticket.pullRequest?.state === "merged") return `merged, last run ${when}`;
  if (ticket.pullRequest?.state === "closed") return `pull request closed, last run ${when}`;
  if (ticket.latest === null || ticket.latest.state === "queued") return `approved ${when}`;
  return `started ${when}`;
}

/** What a ticket asks of a person, in a sentence. A failure says whether it was all of the ticket's runs. */
export function needsText(needs: Needs, runs: number): string {
  if (needs.kind === "question") return `Asks: “${needs.text}”`;
  const times = needs.runs >= runs ? `all ${needs.runs} runs` : `the last ${needs.runs} ${needs.runs === 1 ? "run" : "runs"}`;
  return `Failed: ${needs.step} on ${times}. Fix it, or run it again.`;
}
