import { describe, expect, it } from "vitest";
import type { TicketSummary } from "./api";
import { ago, grouped, groupOf, lastEvent, needsText, type GroupId } from "./board";
import { summary } from "./fixtures";

const now = new Date("2026-11-24T12:00:00Z");
const minutesAgo = (n: number) => new Date(now.getTime() - n * 60_000).toISOString();

function ticket(over: Partial<TicketSummary> = {}, state = "running", at = minutesAgo(10)): TicketSummary {
  return { ...summary, ...over, latest: { id: "w", kind: "implementation", state, at } };
}
const pull = (state: "open" | "merged" | "closed") => ({ number: 7, url: "https://example.test/7", state, checks: "none" as const });
const failed = ticket({ needs: { kind: "failure", step: "format-rust", runs: 3 } }, "failed");
const asking = ticket({ needs: { kind: "question", text: "Which database should the guards be stored in?" } }, "blocked");

describe("which group a ticket is in", () => {
  it("puts a ticket with a question, or a failure nothing has restarted, in Needs you", () => {
    expect(groupOf(asking, now)).toBe("needs");
    expect(groupOf(failed, now)).toBe("needs");
  });

  it("puts a ticket that is queued, running, in review or in CI in In flight", () => {
    expect(groupOf(ticket({}, "queued"), now)).toBe("flight");
    expect(groupOf(ticket({}, "running"), now)).toBe("flight");
    expect(groupOf(ticket({ pullRequest: pull("open") }, "completed"), now)).toBe("flight");
    expect(groupOf({ ...summary, latest: null, workloads: 0 }, now)).toBe("flight");
  });

  it("puts a merged ticket, a closed one and one that settled this week in Shipped this week", () => {
    expect(groupOf(ticket({ pullRequest: pull("merged") }, "completed"), now)).toBe("shipped");
    expect(groupOf(ticket({ pullRequest: pull("closed") }, "completed"), now)).toBe("shipped");
    expect(groupOf(ticket({ settledAt: minutesAgo(60) }, "completed"), now)).toBe("shipped");
  });

  it("leaves off the board a ticket that settled more than a week ago, but not a merged one still to be judged", () => {
    const old = minutesAgo(8 * 24 * 60);
    expect(groupOf(ticket({ settledAt: old }, "completed", old), now)).toBeNull();
    expect(groupOf(ticket({ pullRequest: pull("merged") }, "completed", old), now)).toBe("shipped");
  });
});

describe("the groups", () => {
  it("are in the order Needs you, In flight, Shipped this week, with the most recently changed first, and none that is empty", () => {
    const rows = [
      ticket({ number: 1, title: "old flight" }, "running", minutesAgo(300)),
      ticket({ number: 2, title: "new flight" }, "running", minutesAgo(5)),
      { ...failed, number: 3 },
    ];
    const result = grouped(rows, now);

    expect(result.map((g) => g.title)).toEqual(["Needs you", "In flight"]);
    expect(result[1]!.tickets.map((t) => t.number)).toEqual([2, 1]);
  });

  // A check is trusted once it is seen to fail on a planted violation.
  const failedTicketIsUnderNeedsYou = (place: (t: TicketSummary) => GroupId | null) => place(failed) === "needs";

  it("fails the check that a failed ticket is under Needs you when it is planted elsewhere", () => {
    expect(failedTicketIsUnderNeedsYou((t) => groupOf(t, now))).toBe(true);
    expect(failedTicketIsUnderNeedsYou(() => "flight")).toBe(false);
  });
});

describe("a ticket in words", () => {
  it("says a question and a failure as the ticket asks", () => {
    expect(needsText(asking.needs!, 1)).toBe("Asks: “Which database should the guards be stored in?”");
    expect(needsText(failed.needs!, 3)).toBe("Failed: format-rust on all 3 runs. Fix it, or run it again.");
    expect(needsText({ kind: "failure", step: "test", runs: 2 }, 5)).toBe("Failed: test on the last 2 runs. Fix it, or run it again.");
  });

  it("says how long ago, as a person does", () => {
    expect(ago(now.getTime() - 20_000, now)).toBe("just now");
    expect(ago(now.getTime() - 40 * 60_000, now)).toBe("40 min ago");
    expect(ago(now.getTime() - 2 * 3_600_000, now)).toBe("2 h ago");
    expect(ago(now.getTime() - 30 * 3_600_000, now)).toBe("yesterday");
    expect(ago(now.getTime() - 3 * 86_400_000, now)).toBe("3 days ago");
    expect(ago(0, now)).toBe("at an unknown time");
  });

  it("gives a short account of the last event", () => {
    expect(lastEvent(ticket({}, "running", minutesAgo(12)), now)).toBe("started 12 min ago");
    expect(lastEvent(ticket({}, "queued", minutesAgo(5)), now)).toBe("approved 5 min ago");
    expect(lastEvent({ ...failed, latest: { ...failed.latest!, at: minutesAgo(120) } }, now)).toBe("failed 2 h ago after 3 runs");
    expect(lastEvent(ticket({ settledAt: minutesAgo(60) }, "completed", minutesAgo(90)), now)).toBe("closed 1 h ago");
  });
});
