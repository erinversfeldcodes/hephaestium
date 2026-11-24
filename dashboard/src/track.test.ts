import { describe, expect, it } from "vitest";
import type { PullRequest } from "./api";
import { stageWords, trackOf, type StepState, type Track, type TrackInput, type TrackWorkload } from "./track";

const pull = (state: PullRequest["state"], checks: PullRequest["checks"] = "none"): PullRequest => ({
  number: 7,
  url: "https://example.test/pull/7",
  state,
  checks,
});
const run = (state: string, over: Partial<TrackWorkload> = {}): TrackWorkload => ({ kind: "implementation", state, ...over });
const review = (lens: string, state: string): TrackWorkload => ({ kind: "review", state, lens });
const judge = (state: string): TrackWorkload => ({ kind: "completeness", state });
const states = (track: Track): readonly StepState[] => track.steps.map((s) => s.state);

type Case = { name: string; input: TrackInput; steps: readonly StepState[]; words: string };

const D = "done", N = "now", B = "bad", T = "todo";
const cases: readonly Case[] = [
  { name: "a ticket with no run yet", input: { workloads: [], pullRequest: null, settled: false }, steps: [N, T, T, T, T, T], words: "Waiting for a worker" },
  { name: "a queued run", input: { workloads: [run("queued")], pullRequest: null, settled: false }, steps: [N, T, T, T, T, T], words: "Waiting for a worker" },
  { name: "a run in progress", input: { workloads: [run("running")], pullRequest: null, settled: false }, steps: [N, T, T, T, T, T], words: "Implementing" },
  { name: "a revision in progress", input: { workloads: [run("completed"), run("running", { kind: "revision" })], pullRequest: pull("open"), settled: false }, steps: [N, T, T, T, T, T], words: "Revising" },
  { name: "a run in its gates", input: { workloads: [run("gating")], pullRequest: null, settled: false }, steps: [D, N, T, T, T, T], words: "Running gates" },
  { name: "a run that stopped to ask", input: { workloads: [run("blocked")], pullRequest: null, settled: false }, steps: [N, T, T, T, T, T], words: "Needs your answer" },
  { name: "a run that failed at a gate", input: { workloads: [run("failed", { failedGate: "format-rust" })], pullRequest: null, settled: false }, steps: [D, B, T, T, T, T], words: "Failed at gates, format-rust" },
  { name: "a run that failed outside a gate", input: { workloads: [run("failed")], pullRequest: null, settled: false }, steps: [B, T, T, T, T, T], words: "Failed in the run" },
  { name: "a run that timed out", input: { workloads: [run("timed_out")], pullRequest: null, settled: false }, steps: [B, T, T, T, T, T], words: "Timed out in the run" },
  { name: "a finished run with no pull request and no review", input: { workloads: [run("completed")], pullRequest: null, settled: false }, steps: [D, D, N, T, T, T], words: "Waiting for review" },
  {
    name: "a review with two of three lenses read",
    input: { workloads: [run("completed"), review("a", "completed"), review("b", "completed"), review("c", "running")], pullRequest: pull("open", "passing"), settled: false },
    steps: [D, D, N, T, T, T],
    words: "In review, 2 of 3 lenses read",
  },
  {
    name: "a review that failed",
    input: { workloads: [run("completed"), review("a", "failed")], pullRequest: pull("open"), settled: false },
    steps: [D, D, B, T, T, T],
    words: "A review failed, 0 of 1 lenses read",
  },
  {
    name: "reviews read and checks pending",
    input: { workloads: [run("completed"), review("a", "completed")], pullRequest: pull("open", "pending"), settled: false },
    steps: [D, D, D, N, T, T],
    words: "Waiting for checks",
  },
  {
    name: "reviews read and checks failing",
    input: { workloads: [run("completed"), review("a", "completed")], pullRequest: pull("open", "failing"), settled: false },
    steps: [D, D, D, B, T, T],
    words: "Checks failing",
  },
  {
    name: "reviews read and checks passing",
    input: { workloads: [run("completed"), review("a", "completed")], pullRequest: pull("open", "passing"), settled: false },
    steps: [D, D, D, D, N, T],
    words: "Ready to merge",
  },
  {
    name: "a review of an earlier round does not count once a revision has run",
    input: { workloads: [run("completed"), review("a", "failed"), run("completed", { kind: "revision" })], pullRequest: pull("open", "pending"), settled: false },
    steps: [D, D, N, T, T, T],
    words: "Waiting for review",
  },
  { name: "a merged pull request not yet judged", input: { workloads: [run("completed")], pullRequest: pull("merged", "passing"), settled: false }, steps: [D, D, D, D, D, N], words: "Waiting to be judged" },
  { name: "a merged pull request being judged", input: { workloads: [run("completed"), judge("running")], pullRequest: pull("merged"), settled: false }, steps: [D, D, D, D, D, N], words: "Being judged" },
  { name: "a judging that failed", input: { workloads: [run("completed"), judge("failed")], pullRequest: pull("merged"), settled: false }, steps: [D, D, D, D, D, B], words: "Judging failed" },
  { name: "a pull request closed without merging", input: { workloads: [run("completed")], pullRequest: pull("closed"), settled: false }, steps: [D, D, D, D, B, T], words: "Closed without merging" },
  { name: "a settled ticket", input: { workloads: [run("completed"), judge("completed")], pullRequest: pull("merged"), settled: true }, steps: [D, D, D, D, D, D], words: "Closed" },
];

describe("a ticket's track", () => {
  it.each(cases)("for $name is $words", ({ input, steps, words }) => {
    const track = trackOf(input);
    expect(states(track)).toEqual(steps);
    expect(stageWords(track)).toBe(words);
  });

  it("has the six steps in order", () => {
    expect(trackOf(cases[0]!.input).steps.map((s) => s.label)).toEqual(["Run", "Gates", "Review", "CI", "Merged", "Judged"]);
  });

  it("counts a lens once, however many of its reviews there are", () => {
    const track = trackOf({
      workloads: [run("completed"), review("a", "completed"), review("a", "completed"), review("b", "running")],
      pullRequest: pull("open"),
      settled: false,
    });
    expect(track.detail).toBe("1 of 2 lenses read");
  });

  // A check that a mapping is right must fail on a wrong one, or it shows nothing.
  describe("the check on every stage", () => {
    const wrong = (input: TrackInput): Track => {
      const track = trackOf(input);
      // Planted: a failed step drawn as the current one.
      return { ...track, steps: track.steps.map((s) => (s.state === "bad" ? { ...s, state: "now" as const } : s)) };
    };
    const mismatches = (map: (input: TrackInput) => Track) =>
      cases.filter((c) => JSON.stringify(states(map(c.input))) !== JSON.stringify(c.steps));

    it("finds nothing wrong in the real function", () => {
      expect(mismatches(trackOf)).toEqual([]);
    });

    it("fails on a stage mapped wrong", () => {
      expect(mismatches(wrong).length).toBeGreaterThan(0);
    });
  });
});
