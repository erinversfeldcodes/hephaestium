import type { PullRequest } from "./api";

export type StepId = "run" | "gates" | "review" | "ci" | "merged" | "judged";
export type StepState = "done" | "now" | "bad" | "todo";

const labels: Readonly<Record<StepId, string>> = {
  run: "Run",
  gates: "Gates",
  review: "Review",
  ci: "CI",
  merged: "Merged",
  judged: "Judged",
};

const order: readonly StepId[] = ["run", "gates", "review", "ci", "merged", "judged"];

/** A workload as far as the track needs it. A ticket's workloads are given oldest first. */
export interface TrackWorkload {
  readonly kind: string;
  readonly state: string;
  /** The lens of a review. */
  readonly lens?: string | null;
  /** The gate a failed run stopped at, when it stopped at one. */
  readonly failedGate?: string | null;
}

export interface TrackInput {
  readonly workloads: readonly TrackWorkload[];
  readonly pullRequest: PullRequest | null;
  /** True once the ticket has been judged and closed. */
  readonly settled: boolean;
}

export interface Track {
  readonly steps: readonly { readonly id: StepId; readonly label: string; readonly state: StepState }[];
  /** Where the ticket is, in a few words: "In review". */
  readonly stage: string;
  /** More about where, or null: "2 of 3 lenses read". */
  readonly detail: string | null;
}

/** The line in words that says where a ticket is: "In review, 2 of 3 lenses read". */
export function stageWords(track: Track): string {
  return track.detail === null ? track.stage : `${track.stage}, ${track.detail}`;
}

const failedStates = ["failed", "timed_out", "cancelled"];
const judgingKinds = ["completeness", "gap"];

const isReview = (w: TrackWorkload) => w.kind === "review";
const isJudging = (w: TrackWorkload) => judgingKinds.includes(w.kind);
const isRun = (w: TrackWorkload) => !isReview(w) && !isJudging(w);

/**
 * Where a ticket is on the six steps, from its workloads and its pull request alone. Steps
 * before the current one are done, the current one is `now`, a step that failed is `bad`, and
 * the rest are `todo`. A merged pull request settles every step up to Merged whatever its
 * runs say, as the code is in; a closed one that was not merged fails Merged.
 */
export function trackOf({ workloads, pullRequest, settled }: TrackInput): Track {
  const at = (current: StepId, mark: "now" | "bad", stage: string, detail: string | null = null): Track => {
    const index = order.indexOf(current);
    return {
      steps: order.map((id, i) => ({ id, label: labels[id], state: i < index ? "done" : i === index ? mark : "todo" })),
      stage,
      detail,
    };
  };
  const finished = (stage: string): Track => ({
    steps: order.map((id) => ({ id, label: labels[id], state: "done" as const })),
    stage,
    detail: null,
  });

  if (settled && pullRequest?.state !== "closed") return finished("Closed");
  if (pullRequest?.state === "closed") return at("merged", "bad", "Closed without merging");

  if (pullRequest?.state === "merged") {
    const judging = workloads.filter(isJudging).at(-1);
    if (judging !== undefined && failedStates.includes(judging.state)) return at("judged", "bad", "Judging failed");
    if (judging === undefined || judging.state === "queued") return at("judged", "now", "Waiting to be judged");
    return at("judged", "now", "Being judged");
  }

  const run = workloads.filter(isRun).at(-1);
  if (run === undefined || run.state === "queued") return at("run", "now", "Waiting for a worker");
  const revising = run.kind === "revision";
  switch (run.state) {
    case "preparing":
    case "running":
      return at("run", "now", revising ? "Revising" : "Implementing");
    case "gating":
      return at("gates", "now", "Running gates");
    case "blocked":
      return at("run", "now", "Needs your answer");
    case "failed":
    case "timed_out":
    case "cancelled": {
      const how = run.state === "failed" ? "Failed" : run.state === "timed_out" ? "Timed out" : "Cancelled";
      return run.failedGate ? at("gates", "bad", `${how} at gates`, run.failedGate) : at("run", "bad", `${how} in the run`);
    }
  }

  // The run is over and well: what remains is the review of its change and the pull request's checks.
  const reviews = workloads.slice(workloads.lastIndexOf(run) + 1).filter(isReview);
  const keyed = reviews.map((r, i) => ({ r, key: r.lens ?? `review ${i}` }));
  const lenses = new Set(keyed.map((k) => k.key));
  const read = new Set(keyed.filter((k) => k.r.state === "completed").map((k) => k.key));
  const tally = `${read.size} of ${lenses.size} lenses read`;
  if (reviews.some((r) => failedStates.includes(r.state))) return at("review", "bad", "A review failed", tally);
  if (reviews.some((r) => !["completed", "not_needed", "blocked"].includes(r.state))) {
    return at("review", "now", "In review", tally);
  }
  const checks = pullRequest?.checks ?? "none";
  if (reviews.length === 0 && (pullRequest === null || checks === "pending" || checks === "none")) {
    return at("review", "now", "Waiting for review");
  }
  if (checks === "failing") return at("ci", "bad", "Checks failing");
  if (checks === "passing") return at("merged", "now", "Ready to merge");
  return at("ci", "now", "Waiting for checks");
}
