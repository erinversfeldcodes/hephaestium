import type { Client, Question, RunDetail, ReplyResult, SpendDay, TicketDetail, TicketSummary } from "./api";

export const summary: TicketSummary = {
  repository: "erinversfeldcodes/gazebolang",
  number: 12,
  title: "Teach the parser about guards",
  workloads: 2,
  settledAt: null,
  latest: { id: "w-2", state: "running", kind: "review", at: "2026-11-20T10:00:00Z" },
};

export const detail: TicketDetail = {
  repository: "erinversfeldcodes/gazebolang",
  number: 12,
  title: "Teach the parser about guards",
  url: "https://github.com/erinversfeldcodes/gazebolang/issues/12",
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
    ...overrides,
  };
}
