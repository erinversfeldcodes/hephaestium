import type { Client, DeviceToken, Gate, ReviewEffort, Verdict, Question, RunDetail, ReplyResult, SpendDay, TicketDetail, TicketSummary } from "./api";

export const summary: TicketSummary = {
  repository: "erinversfeldcodes/gazebolang",
  number: 12,
  title: "Teach the parser about guards",
  workloads: 2,
  settledAt: null,
  latest: { id: "w-2", state: "running", kind: "review", at: "2026-11-20T10:00:00Z" },
  waiting: null,
  pullRequest: null,
};

export const detail: TicketDetail = {
  repository: "erinversfeldcodes/gazebolang",
  number: 12,
  title: "Teach the parser about guards",
  url: "https://github.com/erinversfeldcodes/gazebolang/issues/12",
  pullRequest: null,
  workloads: [
    {
      id: "w-1",
      kind: "implementation",
      state: "succeeded",
      approvedBy: "octocat",
      createdAt: "2026-11-19T09:00:00Z",
      startedAt: "2026-11-19T09:01:00Z",
      finishedAt: "2026-11-19T09:30:00Z",
    },
    {
      id: "w-2",
      kind: "review",
      state: "running",
      approvedBy: null,
      createdAt: "2026-11-20T10:00:00Z",
      startedAt: "2026-11-20T10:00:05Z",
      finishedAt: null,
    },
  ],
  spend: {
    totals: { usd: 1.5, tokens: 12345 },
    byKind: { implementation: { usd: 1.25 }, review: { usd: 0.25 } },
    workloads: 2,
  },
};

export const question: Question = {
  workload: "w-3",
  repository: "erinversfeldcodes/gazebolang",
  ticket: 12,
  title: "Teach the parser about guards",
  question: "Which database should the guards be stored in?",
  askedAt: "2026-11-20T11:00:00Z",
};

export const runDetail: RunDetail = {
  id: "w-2",
  repository: "erinversfeldcodes/gazebolang",
  ticket: 12,
  kind: "review",
  state: "running",
  startedAt: "2026-11-20T10:00:05Z",
  finishedAt: null,
};

export const replied: ReplyResult = { commented: true, approved: true, error: null };

export const days: SpendDay[] = [
  { day: "2026-11-20", totals: { usd: 0.25, tokens: 2000 } },
  { day: "2026-11-19", totals: { usd: 1.25, tokens: 10345 } },
];

export const deviceTokens: DeviceToken[] = [
  {
    id: "t-1",
    name: "laptop",
    createdAt: "2026-11-18T08:00:00Z",
    lastSeenAt: "2026-11-23T09:00:00Z",
    revokedAt: null,
    connected: true,
  },
  {
    id: "t-2",
    name: "desk",
    createdAt: "2026-11-10T08:00:00Z",
    lastSeenAt: null,
    revokedAt: null,
    connected: false,
  },
];

export const gates: Gate[] = [
  {
    name: "format",
    outcome: "passed",
    reason: null,
    ciCheck: null,
    durationMs: 40,
  },
  {
    name: "test",
    outcome: "deferred",
    reason: "timed out after 60s",
    ciCheck: "ci / test",
    durationMs: 60000,
  },
];

export const verdicts: Verdict[] = [
  {
    workload: "w-9",
    at: "2026-11-21T10:00:00Z",
    verdict: "not_met",
    requirements: [
      {
        status: "not_met",
        text: "Guards parse",
        evidenceKind: "test",
        evidence: "no test covers guards",
      },
    ],
  },
  {
    workload: "w-10",
    at: "2026-11-22T10:00:00Z",
    verdict: "met",
    requirements: [
      {
        status: "met",
        text: "Guards parse",
        evidenceKind: "test",
        evidence: "guards_test passes",
      },
    ],
  },
];

export const reviewEffort: ReviewEffort = {
  localRounds: 2,
  reviews: 3,
  revisions: 1,
  lenses: [
    {
      lens: "security",
      reviews: 2,
      found: 2,
      resolved: 1,
      remaining: 1,
      findings: [
        {
          title: "leaks a key",
          severity: "high",
          blocking: true,
          state: "resolved",
        },
        {
          title: "naming",
          severity: "low",
          blocking: false,
          state: "remaining",
        },
      ],
    },
  ],
};

/** A client whose answers are these fixtures, unless a test says otherwise. */
export function fakeClient(overrides: Partial<Client> = {}): Client {
  return {
    session: async () => ({ login: "octocat" }),
    tickets: async () => [summary],
    ticket: async () => detail,
    spendDays: async () => days,
    question: async () => null,
    questions: async () => [],
    workload: async () => runDetail,
    reply: async () => replied,
    agentText: async () => ({ account: null, question: null }),
    gates: async () => gates,
    verdicts: async () => verdicts,
    reviewEffort: async () => reviewEffort,
    deviceTokens: async () => deviceTokens,
    makeDeviceToken: async (name) => ({
      ok: true,
      value: {
        token: { id: "t-3", name, createdAt: "2026-11-23T10:00:00Z", lastSeenAt: null, revokedAt: null, connected: false },
        secret: "heph_secret_once",
      },
    }),
    revokeDeviceToken: async () => ({ ok: true, value: null }),
    ...overrides,
  };
}
