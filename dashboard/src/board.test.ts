import { describe, expect, it } from "vitest";
import type { TicketSummary } from "./api";
import { columnOf, columns, lanes, waitedFor } from "./board";
import { summary } from "./fixtures";

function ticket(state: string, over: Partial<TicketSummary> = {}): TicketSummary {
  return { ...summary, ...over, latest: { ...summary.latest!, state } };
}

describe("which column a ticket is in", () => {
  it("follows the state of its latest run", () => {
    expect(columnOf(ticket("queued"))).toBe("waiting");
    for (const state of ["preparing", "running", "gating"]) expect(columnOf(ticket(state))).toBe("working");
    expect(columnOf(ticket("blocked"))).toBe("blocked");
    for (const state of ["failed", "timed_out", "cancelled"]) expect(columnOf(ticket(state))).toBe("failed");
    expect(columnOf(ticket("completed"))).toBe("completed");
  });

  it("puts an approved ticket that has no run yet in the waiting column", () => {
    expect(columnOf({ ...summary, latest: null, workloads: 0, waiting: { since: null, approvedBy: "maint", noWorker: false } })).toBe(
      "waiting",
    );
  });

  it("is done once the ticket has settled, whatever its last run said", () => {
    expect(columnOf(ticket("completed", { settledAt: "2026-11-21T00:00:00Z" }))).toBe("done");
  });

  it("puts a state it does not know in a column of its own, so no ticket goes missing", () => {
    expect(columnOf(ticket("invented"))).toBe("unknown");
  });

  it("orders the columns from approved to done", () => {
    expect(columns.map((c) => c.id)).toEqual(["waiting", "working", "blocked", "failed", "completed", "done", "unknown"]);
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
    expect(result[1]?.cards.get("waiting")?.map((t) => t.number)).toEqual([2]);
  });
});

describe("how long a ticket has waited", () => {
  const now = new Date("2026-11-23T12:00:00Z");

  it("is said in the largest unit that fits", () => {
    expect(waitedFor("2026-11-23T11:59:50Z", now)).toBe("waiting less than a minute");
    expect(waitedFor("2026-11-23T11:55:00Z", now)).toBe("waiting 5 minutes");
    expect(waitedFor("2026-11-23T09:00:00Z", now)).toBe("waiting 3 hours");
    expect(waitedFor("2026-11-20T12:00:00Z", now)).toBe("waiting 3 days");
  });

  it("is unknown where the control plane holds no time, and never guessed", () => {
    expect(waitedFor(null, now)).toBe("wait unknown");
    expect(waitedFor("not a time", now)).toBe("wait unknown");
  });
});
