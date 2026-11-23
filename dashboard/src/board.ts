import type { TicketSummary } from "./api";

export type ColumnId = "queued" | "working" | "blocked" | "failed" | "completed" | "done" | "unknown";

/** The columns, from a ticket approved and waiting to a ticket settled. */
export const columns: readonly { readonly id: ColumnId; readonly title: string }[] = [
  { id: "queued", title: "Queued" },
  { id: "working", title: "Working" },
  { id: "blocked", title: "Blocked" },
  { id: "failed", title: "Failed" },
  { id: "completed", title: "Completed" },
  { id: "done", title: "Done" },
  { id: "unknown", title: "Other" },
];

/**
 * Where a ticket stands: by the state of its latest run, unless it has settled. A state this
 * does not know goes to its own column, so a new state on the API shows up instead of vanishing.
 */
export function columnOf(ticket: TicketSummary): ColumnId {
  if (ticket.settledAt !== null) return "done";
  switch (ticket.latest.state) {
    case "queued":
      return "queued";
    case "preparing":
    case "running":
    case "gating":
      return "working";
    case "blocked":
      return "blocked";
    case "failed":
    case "timed_out":
    case "cancelled":
      return "failed";
    case "completed":
      return "completed";
    default:
      return "unknown";
  }
}

export interface Lane {
  readonly repository: string;
  readonly cards: ReadonlyMap<ColumnId, readonly TicketSummary[]>;
}

/** One lane for each repository, in the order its first ticket came. */
export function lanes(tickets: readonly TicketSummary[]): readonly Lane[] {
  const byRepository = new Map<string, Map<ColumnId, TicketSummary[]>>();
  for (const ticket of tickets) {
    const cards = byRepository.get(ticket.repository) ?? new Map<ColumnId, TicketSummary[]>();
    const column = columnOf(ticket);
    cards.set(column, [...(cards.get(column) ?? []), ticket]);
    byRepository.set(ticket.repository, cards);
  }
  return [...byRepository].map(([repository, cards]) => ({ repository, cards }));
}
