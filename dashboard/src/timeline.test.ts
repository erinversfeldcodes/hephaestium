import { describe, expect, it } from "vitest";
import type { Run } from "./api";
import { runWord } from "./runwords";
import { timelineOf } from "./timeline";

const T0 = Date.parse("2026-11-19T09:00:00Z");
const at = (minutes: number) => new Date(T0 + minutes * 60_000).toISOString();

function run(id: string, kind: string, state: string, from: number, to: number | null): Run {
  return { id, kind, state, approvedBy: null, createdAt: at(from), startedAt: at(from), finishedAt: to === null ? null : at(to) };
}

const now = T0 + 200 * 60_000;

describe("the runs' timeline", () => {
  it("has a lane for each run, in the order they began", () => {
    const { lanes } = timelineOf([run("b", "review", "completed", 45, 50), run("a", "implementation", "completed", 0, 40)], now, false);
    expect(lanes.map((l) => l.id)).toEqual(["a", "b"]);
    expect(lanes.map((l) => l.label)).toEqual(["Implementation", "Review"]);
  });

  it("puts each bar where its run began and as wide as its run lasted, on one axis", () => {
    // The axis reaches 60 min, the end of the last run, in round 15-minute ticks.
    const { lanes, axis } = timelineOf([run("a", "implementation", "completed", 0, 40), run("b", "review", "completed", 40, 60)], now, false);
    expect(axis.ticks.map((t) => t.label)).toEqual(["0", "15 min", "30 min", "45 min", "1 h"]);
    expect(lanes[0]!.left).toBeCloseTo(0);
    expect(lanes[0]!.width).toBeCloseTo((40 / 60) * 100);
    expect(lanes[1]!.left).toBeCloseTo((40 / 60) * 100);
    expect(lanes[1]!.width).toBeCloseTo((20 / 60) * 100);
    expect(lanes.map((l) => l.duration)).toEqual(["40 min", "20 min"]);
  });

  it("colours a bar by its run's state: running, failed, finished", () => {
    const { lanes } = timelineOf(
      [run("a", "implementation", "failed", 0, 10), run("b", "revision", "running", 10, null), run("c", "review", "completed", 11, 12)],
      T0 + 20 * 60_000,
      false,
    );
    expect(lanes.map((l) => l.state.tone)).toEqual(["bad", "running", "ok"]);
  });

  it("draws a break when two runs are more than an hour apart, and says how long and why", () => {
    const { lanes } = timelineOf([run("a", "implementation", "completed", 0, 40), run("b", "completeness", "completed", 40 + 255, 40 + 258)], now, true);
    expect(lanes[0]!.gap).toBeNull();
    expect(lanes[1]!.gap?.text).toBe("4 h 15 m later, after the merge");
    expect(lanes[1]!.left).toBeGreaterThan(lanes[0]!.left + lanes[0]!.width);
    expect(lanes[1]!.left + lanes[1]!.width).toBeLessThanOrEqual(100);
  });

  it("draws no break at an hour exactly or less", () => {
    const { lanes } = timelineOf([run("a", "implementation", "completed", 0, 10), run("b", "review", "completed", 70, 75)], now, false);
    expect(lanes.every((l) => l.gap === null)).toBe(true);
  });

  it("does not say after the merge when nothing merged", () => {
    const { lanes } = timelineOf([run("a", "implementation", "completed", 0, 10), run("b", "review", "completed", 200, 205)], now, false);
    expect(lanes[1]!.gap?.text).toBe("3 h 10 m later");
  });

  it("labels the axis only where the scale is true", () => {
    const { axis } = timelineOf([run("a", "implementation", "completed", 0, 40), run("b", "completeness", "completed", 300, 303)], now, true);
    expect(axis.ticks.map((t) => t.label)).toEqual(["0", "10 min", "20 min", "30 min", "40 min"]);
  });
});

describe("a run's words", () => {
  it.each([
    ["queued", "waiting"],
    ["running", "running"],
    ["completed", "finished"],
    ["failed", "failed"],
    ["cancelled", "stopped"],
    ["timed_out", "timed out"],
  ])("calls %s %s", (state, word) => {
    expect(runWord(state).word).toBe(word);
    expect(runWord(state).meaning).not.toBe("");
  });

  it("never calls a run done", () => {
    for (const state of ["queued", "preparing", "running", "gating", "blocked", "completed", "failed", "cancelled", "timed_out", "done"]) {
      expect(runWord(state).word).not.toMatch(/^done$/i);
    }
  });
});
