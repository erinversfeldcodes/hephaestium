// The control plane's answers, written from the fixtures the dashboard's other tests already
// use. The fixtures hold what the client reads; this writes what the API sends, the same fields
// under their wire names, so the real client reads them back as the fixtures. A test in src
// proves the round trip, so a field one side renames cannot leave the other behind.
import type { DeviceToken, PullRequest, Question, TicketDetail, TicketSummary } from "../src/api";
import {
  days,
  deviceTokens,
  detail,
  gates,
  question,
  reviewEffort,
  runDetail,
  summary,
  verdicts,
} from "../src/fixtures";

export const pullRequest: PullRequest = {
  number: 14,
  url: "https://github.com/erinversfeldcodes/gazebolang/pull/14",
  state: "open",
  checks: "passing",
};

/** The data a page is shown from: the fixtures, with a pull request and a waiting question so those parts of a page are on screen. */
export interface World {
  readonly tickets: readonly TicketSummary[];
  readonly detail: TicketDetail;
  readonly questions: readonly Question[];
  readonly tokens: readonly DeviceToken[];
}

export const world: World = {
  tickets: [
    { ...summary, pullRequest },
    {
      ...summary,
      number: 13,
      title: "A ticket that waits for a worker",
      workloads: 0,
      latest: null,
      runs: [],
      spendUsd: 0,
      waiting: { since: "2026-11-20T09:00:00Z", approvedBy: "octocat", noWorker: true },
    },
    {
      ...summary,
      number: 14,
      title: "A ticket whose runs failed and a ticket with a very long title that has to wrap on a narrow screen",
      workloads: 3,
      latest: { id: "w-9", state: "failed", kind: "implementation", at: "2026-11-20T08:00:00Z" },
      runs: [{ kind: "implementation", state: "failed", lens: null, at: "2026-11-20T08:00:00Z", failedGate: "format-rust" }],
      needs: { kind: "failure", step: "format-rust", runs: 3 },
    },
  ],
  detail: { ...detail, pullRequest },
  questions: [question],
  tokens: deviceTokens,
};

export function wireTickets(tickets: readonly TicketSummary[]) {
  return {
    tickets: tickets.map((t) => ({
      repository: t.repository,
      number: t.number,
      title: t.title,
      workloads: t.workloads,
      settled_at: t.settledAt,
      latest: t.latest,
      waiting: t.waiting && {
        since: t.waiting.since,
        approved_by: t.waiting.approvedBy,
        no_worker: t.waiting.noWorker,
      },
      pull_request: t.pullRequest,
      runs: t.runs.map((r) => ({ kind: r.kind, state: r.state, lens: r.lens, at: r.at, failed_gate: r.failedGate })),
      spend_usd: t.spendUsd,
      needs: t.needs,
    })),
  };
}

export function wireTicket(t: TicketDetail) {
  return {
    repository: t.repository,
    number: t.number,
    title: t.title,
    url: t.url,
    pull_request: t.pullRequest,
    workloads: t.workloads.map((w) => ({
      id: w.id,
      kind: w.kind,
      state: w.state,
      approved_by: w.approvedBy,
      created_at: w.createdAt,
      started_at: w.startedAt,
      finished_at: w.finishedAt,
    })),
    spend: { totals: t.spend.totals, by_kind: t.spend.byKind, workloads: t.spend.workloads },
  };
}

export function wireRun() {
  return {
    id: runDetail.id,
    repository: runDetail.repository,
    ticket_number: runDetail.ticket,
    kind: runDetail.kind,
    state: runDetail.state,
    started_at: runDetail.startedAt,
    finished_at: runDetail.finishedAt,
  };
}

export function wireQuestions(questions: readonly Question[]) {
  return {
    questions: questions.map((q) => ({
      workload: q.workload,
      repository: q.repository,
      ticket: q.ticket,
      title: q.title,
      question: q.question,
      asked_at: q.askedAt,
    })),
  };
}

export function wireGates() {
  return {
    gates: gates.map((g) => ({
      name: g.name,
      outcome: g.outcome,
      reason: g.reason,
      ci_check: g.ciCheck,
      duration_ms: g.durationMs,
    })),
  };
}

export function wireVerdicts() {
  return {
    verdicts: verdicts.map((v) => ({
      workload: v.workload,
      at: v.at,
      verdict: {
        verdict: v.verdict,
        requirements: v.requirements.map((r) => ({
          status: r.status,
          text: r.text,
          evidence: { kind: r.evidenceKind, detail: r.evidence },
        })),
      },
    })),
  };
}

export function wireReviewEffort() {
  return {
    local_rounds: reviewEffort.localRounds,
    reviews: reviewEffort.reviews,
    revisions: reviewEffort.revisions,
    lenses: reviewEffort.lenses,
  };
}

export function wireDays() {
  return { days };
}

export function wireTokens(tokens: readonly DeviceToken[]) {
  return { tokens };
}

/** The events of a run as the stream writes them: one frame each, ending with the one that finishes the run. */
export function eventStream(): string {
  const events = [
    { kind: "started", event: { started: { executorVersion: "1.0.0" } } },
    { kind: "workspaceReady", event: { workspaceReady: {} } },
    { kind: "agentMessage", event: { agentMessage: { text: "Reading the parser to see where guards belong." } } },
    { kind: "finished", event: { finished: { outcome: "OUTCOME_SUCCEEDED" } } },
  ];
  return events
    .map((e, i) => `id: ${i + 1}\ndata: ${JSON.stringify({ sequence: i + 1, time: "2026-11-20T10:00:06Z", ...e })}\n\n`)
    .join("");
}
