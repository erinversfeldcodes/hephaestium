import { describe, expect, it } from "vitest";
import type { TicketSummary } from "./api";
import { columnOf, columns, lanes } from "./board";
import { summary } from "./fixtures";

function ticket(state: string, over: Partial<TicketSummary> = {}): TicketSummary {
  return { ...summary, ...over, latest: { ...summary.latest, state } };
}

describe("which column a ticket is in", () => {
  it("follows the state of its latest run", () => {
    expect(columnOf(ticket("queued"))).toBe("queued");
    for (const state of ["preparing", "running", "gating"]) expect(columnOf(ticket(state))).toBe("working");
    expect(columnOf(ticket("blocked"))).toBe("blocked");
    for (const state of ["failed", "timed_out", "cancelled"]) expect(columnOf(ticket(state))).toBe("failed");
    expect(columnOf(ticket("completed"))).toBe("completed");
  });

  it("is done once the ticket has settled, whatever its last run said", () => {
    expect(columnOf(ticket("completed", { settledAt: "2026-11-21T00:00:00Z" }))).toBe("done");
  });

  it("puts a state it does not know in a column of its own, so no ticket goes missing", () => {
    expect(columnOf(ticket("invented"))).toBe("unknown");
  });

  it("orders the columns from approved to done", () => {
    expect(columns.map((c) => c.id)).toEqual(["queued", "working", "blocked", "failed", "completed", "done", "unknown"]);
  });
});

describe("the lanes", () => {
  it("are one for each repository, in the order the tickets came, each ticket in its column", () => {
    const rows = [
      ticket("running", { number: 1 }),
      ticket("queued", { number: 2, repository: "o/other" }),
      ticket("blocked", { number: 3 }),
    ];

    const result = lanes(rows);

    expect(result.map((l) => l.repository)).toEqual(["erinversfeldcodes/gazebolang", "o/other"]);
    expect(result[0]?.cards.get("working")?.map((t) => t.number)).toEqual([1]);
    expect(result[0]?.cards.get("blocked")?.map((t) => t.number)).toEqual([3]);
    expect(result[1]?.cards.get("queued")?.map((t) => t.number)).toEqual([2]);
  });
});
