// The dashboard reads the control plane's API, and writes two things: a person's reply to an
// agent's question, and their own device tokens (making one, revoking one). Every request is a
// GET (through `get`) except those POSTs, which are made through `write` alone, each to the one
// address it has, with the CSRF token the API gave for the session. A person's session is the cookie the browser already holds.
//
// What the API answers is checked as it is read: a body that is not the shape promised is an
// error, never a half-built value for a page to trip over. A workload's `reason` is not read
// at all, since it can carry what an agent said. What an agent said is read from its own route,
// and only when a person asks for it.

export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
  }
}

export interface Session {
  readonly login: string;
}

/** What was spent, each unit on its own: dollars and tokens are never added to each other. */
export type Totals = Readonly<Record<string, number>>;

/** A ticket's pull request as the control plane last recorded it, never read from the code host. */
export interface PullRequest {
  readonly number: number;
  readonly url: string;
  readonly state: "open" | "merged" | "closed";
  readonly checks: "passing" | "failing" | "pending" | "none";
}

export interface TicketSummary {
  readonly repository: string;
  readonly number: number;
  readonly title: string | null;
  readonly workloads: number;
  readonly settledAt: string | null;
  /** The latest run, or null for an approved ticket that has no run yet. */
  readonly latest: {
    readonly id: string;
    readonly state: string;
    readonly kind: string;
    readonly at: string;
  } | null;
  /** Present while the ticket waits for a run to start: no workload yet, or a queued one. */
  readonly waiting: Waiting | null;
  /** The ticket's pull request, or null when it has none. */
  readonly pullRequest: PullRequest | null;
  /** The ticket's workloads, oldest first: what its track is drawn from. */
  readonly runs: readonly TrackRun[];
  /** What the ticket's runs have cost, in dollars. */
  readonly spendUsd: number;
  /** What the ticket asks of a person, or null when it asks nothing. */
  readonly needs: Needs | null;
}

/** One workload of a ticket, as far as the track needs to know it. */
export interface TrackRun {
  readonly kind: string;
  readonly state: string;
  /** The lens of a review, or null for any other workload. */
  readonly lens: string | null;
  readonly at: string;
  /** The gate a failed run stopped at, or null when it stopped at none. */
  readonly failedGate: string | null;
}

/** What a ticket needs from a person: its question, or the step its runs failed at. */
export type Needs =
  | { readonly kind: "question"; readonly text: string }
  | { readonly kind: "failure"; readonly step: string; readonly runs: number };

/** A ticket waiting for work to start. `since` is null where the control plane does not hold the time. */
export interface Waiting {
  readonly since: string | null;
  readonly approvedBy: string | null;
  /** True for a queued workload that no worker has claimed, as no executor of its approver is connected. */
  readonly noWorker: boolean;
}

export interface Run {
  readonly id: string;
  readonly kind: string;
  readonly state: string;
  readonly approvedBy: string | null;
  readonly createdAt: string;
  readonly startedAt: string | null;
  readonly finishedAt: string | null;
  /** The lens of a review run, when the control plane says it. */
  readonly lens?: string | null;
}

/** One run, as the run's page shows it. Its `reason` is not here: see the note at the top. */
export interface RunDetail {
  readonly id: string;
  readonly repository: string;
  readonly ticket: number;
  readonly kind: string;
  readonly state: string;
  readonly startedAt: string | null;
  readonly finishedAt: string | null;
}

export interface TicketDetail {
  readonly repository: string;
  readonly number: number;
  readonly title: string | null;
  readonly url: string | null;
  /** The ticket's pull request, or null when it has none. */
  readonly pullRequest: PullRequest | null;
  readonly workloads: readonly Run[];
  readonly spend: {
    readonly totals: Totals;
    readonly byKind: Readonly<Record<string, Totals>>;
    readonly workloads: number;
  };
}

export interface SpendDay {
  readonly day: string;
  readonly totals: Totals;
}

/** One gate a run ran: what it was and what it came to. Nothing in it is an agent's text. */
export interface Gate {
  readonly name: string;
  readonly outcome: string | null;
  readonly reason: string | null;
  readonly ciCheck: string | null;
  readonly durationMs: number | null;
}

/** One completeness verdict of a ticket, as given. The requirements are the agent's words. */
export interface Verdict {
  readonly workload: string;
  readonly at: string | null;
  readonly verdict: string;
  readonly requirements: readonly {
    readonly status: string;
    readonly text: string | null;
    readonly evidenceKind: string | null;
    readonly evidence: string | null;
  }[];
}

/** One finding a lens made, and whether the lens still makes it. */
export interface LensFinding {
  readonly title: string;
  readonly severity: string | null;
  readonly blocking: boolean;
  readonly state: "resolved" | "remaining";
}

/** What a ticket's reviewing came to under one lens. */
export interface LensEffort {
  readonly lens: string;
  readonly reviews: number;
  readonly found: number;
  readonly resolved: number;
  readonly remaining: number;
  readonly findings: readonly LensFinding[];
}

/** How much reviewing a ticket needed. */
export interface ReviewEffort {
  readonly localRounds: number;
  readonly reviews: number;
  readonly revisions: number;
  readonly lenses: readonly LensEffort[];
}

/** A run's question, waiting for a person to answer it. */
export interface Question {
  readonly workload: string;
  readonly repository: string;
  readonly ticket: number;
  readonly title: string | null;
  readonly question: string;
  readonly askedAt: string | null;
}

/**
 * What a reply did. `commented` and `approved` are what was made on the ticket, as the person;
 * `error` is the API's own wording for what was not, and is null when both were.
 */
export interface ReplyResult {
  readonly commented: boolean;
  readonly approved: boolean;
  readonly error: string | null;
}

/** What an agent wrote in one run: its last message, and the question it stopped to ask. */
export interface AgentText {
  readonly account: string | null;
  readonly question: string | null;
}

/** One of a person's device tokens. The token itself is never here: it is shown once, on making. */
export interface DeviceToken {
  readonly id: string;
  readonly name: string;
  readonly createdAt: string;
  readonly lastSeenAt: string | null;
  readonly revokedAt: string | null;
  /** Whether a worker with this token is connected now. */
  readonly connected: boolean;
  /** How many workloads this worker holds a lease on now. */
  readonly running: number;
  /** How many workloads this worker has said it may run at once. */
  readonly slots: number;
}

/** A device token just made. `secret` is the only time it can be read. */
export interface IssuedToken {
  readonly token: DeviceToken;
  readonly secret: string;
}

/** What a write to a device token did: its value, or the reason it was not done. */
export type Written<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly error: string };

export interface Client {
  /** Who is signed in, or null when nobody is. Any other failure raises. */
  session(): Promise<Session | null>;
  /** The tickets that have had runs, the most recently active first. */
  tickets(): Promise<readonly TicketSummary[]>;
  /** One run, or null when there is none by that id. */
  workload(id: string): Promise<RunDetail | null>;
  /** One ticket, or null when it has had no run. */
  ticket(repository: string, number: number): Promise<TicketDetail | null>;
  /** What a repository spent on each of the last days, newest first. */
  spendDays(repository: string, days: number): Promise<readonly SpendDay[]>;
  /** The question waiting for a person on a ticket, or null when none is. */
  question(repository: string, number: number): Promise<Question | null>;
  /** Every question waiting for a person, on any ticket. */
  questions(): Promise<readonly Question[]>;
  /** Answer a ticket's question as the signed-in person. Never raises for a refusal. */
  reply(repository: string, number: number, body: string): Promise<ReplyResult>;
  /** What the agent said in one run. It is asked for only when a person asks to read it. */
  agentText(workloadId: string): Promise<AgentText>;
  /** The gates a run ran, in the order the run kept them. */
  gates(workloadId: string): Promise<readonly Gate[]>;
  /** Every completeness verdict a ticket has had, oldest first. */
  verdicts(repository: string, number: number): Promise<readonly Verdict[]>;
  /** How much reviewing a ticket needed. */
  reviewEffort(repository: string, number: number): Promise<ReviewEffort>;
  /** The signed-in person's device tokens, newest first as the API gives them. */
  deviceTokens(): Promise<readonly DeviceToken[]>;
  /** Make a device token. Never raises for a refusal. */
  makeDeviceToken(name: string): Promise<Written<IssuedToken>>;
  /** Revoke one of the person's device tokens. Never raises for a refusal. */
  revokeDeviceToken(id: string): Promise<Written<null>>;
}

type Fetcher = typeof fetch;

export function createClient(fetcher: Fetcher = (...args) => fetch(...args)): Client {
  async function get(path: string): Promise<Response> {
    return fetcher(path, { method: "GET", credentials: "same-origin" });
  }

  async function read<T>(path: string, parse: (body: unknown) => T): Promise<T> {
    const response = await get(path);
    if (!response.ok) throw new ApiError(response.status, response.statusText);
    return parse(await response.json());
  }

  const allQuestions = () => read("/api/questions", parseQuestions);

  return {
    async session() {
      const response = await get("/api/session");
      if (response.status === 401) return null;
      if (!response.ok) throw new ApiError(response.status, response.statusText);
      return parseSession(await response.json());
    },

    tickets: () => read("/api/tickets?limit=200", parseTickets),

    async workload(id) {
      const response = await get(`/api/workloads/${encodeURIComponent(id)}`);
      if (response.status === 404) return null;
      if (!response.ok) throw new ApiError(response.status, response.statusText);
      return parseRun(await response.json());
    },

    async ticket(repository, number) {
      const response = await get(`/api/tickets/${repository}/${number}`);
      if (response.status === 404) return null;
      if (!response.ok) throw new ApiError(response.status, response.statusText);
      return parseTicket(await response.json());
    },

    spendDays: (repository, days) =>
      read(`/api/spend/days?repository=${encodeURIComponent(repository)}&days=${days}`, parseDays),

    questions: allQuestions,

    async question(repository, number) {
      const waiting = await allQuestions();
      return waiting.find((q) => q.repository === repository && q.ticket === number) ?? null;
    },

    async reply(repository, number, body) {
      const token = await csrfToken();
      if (token === null) {
        return unsent("Your session cannot make this reply: sign in again.");
      }
      const response = await fetcher(`/api/tickets/${repository}/${number}/reply`, {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json", "x-csrf-token": token },
        body: JSON.stringify({ body }),
      });
      return parseReply(response);
    },

    agentText: (workloadId) =>
      read(`/api/workloads/${encodeURIComponent(workloadId)}/account`, parseAgentText),

    gates: (workloadId) =>
      read(
        `/api/workloads/${encodeURIComponent(workloadId)}/gates`,
        parseGates,
      ),

    verdicts: (repository, number) =>
      read(`/api/tickets/${repository}/${number}/verdicts`, parseVerdicts),

    reviewEffort: (repository, number) =>
      read(
        `/api/tickets/${repository}/${number}/review-effort`,
        parseReviewEffort,
      ),

    deviceTokens: () => read("/api/device-tokens", parseDeviceTokens),

    makeDeviceToken: (name) =>
      write("/api/device-tokens", { name }, (body) => {
        const secret = text(fields(body)["token"]);
        if (secret === "") throw shapeError();
        return { token: parseDeviceToken(body), secret };
      }),

    revokeDeviceToken: (id) =>
      write(`/api/device-tokens/${encodeURIComponent(id)}/revoke`, {}, () => null),
  };

  async function write<T>(path: string, payload: unknown, parse: (body: unknown) => T): Promise<Written<T>> {
    const token = await csrfToken();
    if (token === null) return { ok: false, error: "Your session cannot make this change: sign in again." };
    try {
      const response = await fetcher(path, {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json", "x-csrf-token": token },
        body: JSON.stringify(payload),
      });
      const body: unknown = await response.json().catch(() => null);
      if (!response.ok) {
        const error = body !== null && typeof body === "object" ? (body as Fields)["error"] : null;
        return { ok: false, error: typeof error === "string" ? error : "The change was not made." };
      }
      return { ok: true, value: parse(body) };
    } catch {
      return { ok: false, error: "The change could not be sent, and may not have been made." };
    }
  }

  async function csrfToken(): Promise<string | null> {
    const response = await get("/api/csrf");
    if (!response.ok) return null;
    try {
      const token = fields(await response.json())["csrfToken"];
      return typeof token === "string" && token !== "" ? token : null;
    } catch {
      return null;
    }
  }
}

function shapeError(): ApiError {
  return new ApiError(200, "an answer is not in the shape the API promises");
}

type Fields = Readonly<Record<string, unknown>>;

function fields(value: unknown): Fields {
  if (typeof value === "object" && value !== null && !Array.isArray(value)) {
    return value as Fields;
  }
  throw shapeError();
}

function list(value: unknown): readonly unknown[] {
  if (Array.isArray(value)) return value as readonly unknown[];
  throw shapeError();
}

function text(value: unknown): string {
  if (typeof value === "string") return value;
  throw shapeError();
}

function count(value: unknown): number {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  throw shapeError();
}

function maybeText(value: unknown): string | null {
  return value === null || value === undefined ? null : text(value);
}

function totals(value: unknown): Totals {
  const entries = Object.entries(fields(value)).map(([unit, amount]) => [unit, count(amount)] as const);
  return Object.fromEntries(entries);
}

function parseSession(body: unknown): Session {
  return { login: text(fields(body)["login"]) };
}

const pullStates = ["open", "merged", "closed"] as const;
const pullChecks = ["passing", "failing", "pending", "none"] as const;

function parsePullRequest(value: unknown): PullRequest | null {
  if (value === null || value === undefined) return null;
  const pull = fields(value);
  const state = pullStates.find((s) => s === pull["state"]);
  const checks = pullChecks.find((c) => c === pull["checks"]);
  if (state === undefined || checks === undefined) throw new Error("a pull request has a state or checks that is not known");
  return { number: count(pull["number"]), url: text(pull["url"]), state, checks };
}

function parseTickets(body: unknown): readonly TicketSummary[] {
  return list(fields(body)["tickets"]).map((raw) => {
    const row = fields(raw);
    const latest = row["latest"] === null || row["latest"] === undefined ? null : fields(row["latest"]);
    const waiting = row["waiting"] === null || row["waiting"] === undefined ? null : fields(row["waiting"]);
    return {
      repository: text(row["repository"]),
      number: count(row["number"]),
      title: maybeText(row["title"]),
      workloads: count(row["workloads"]),
      settledAt: maybeText(row["settled_at"]),
      latest: latest && {
        id: text(latest["id"]),
        state: text(latest["state"]),
        kind: text(latest["kind"]),
        at: text(latest["at"]),
      },
      waiting: waiting && {
        since: maybeText(waiting["since"]),
        approvedBy: maybeText(waiting["approved_by"]),
        noWorker: waiting["no_worker"] === true,
      },
      pullRequest: parsePullRequest(row["pull_request"]),
      runs: list(row["runs"]).map((raw) => {
        const run = fields(raw);
        return {
          kind: text(run["kind"]),
          state: text(run["state"]),
          lens: maybeText(run["lens"]),
          at: text(run["at"]),
          failedGate: maybeText(run["failed_gate"]),
        };
      }),
      spendUsd: typeof row["spend_usd"] === "number" ? row["spend_usd"] : 0,
      needs: parseNeeds(row["needs"]),
    };
  });
}

function parseNeeds(raw: unknown): Needs | null {
  if (raw === null || raw === undefined) return null;
  const needs = fields(raw);
  if (needs["kind"] === "question") return { kind: "question", text: text(needs["text"]) };
  if (needs["kind"] === "failure") return { kind: "failure", step: text(needs["step"]), runs: count(needs["runs"]) };
  throw new Error("a need of a kind this dashboard does not know");
}

function parseRun(body: unknown): RunDetail {
  const run = fields(body);
  return {
    id: text(run["id"]),
    repository: text(run["repository"]),
    ticket: count(run["ticket_number"]),
    kind: text(run["kind"]),
    state: text(run["state"]),
    startedAt: maybeText(run["started_at"]),
    finishedAt: maybeText(run["finished_at"]),
  };
}

function parseTicket(body: unknown): TicketDetail {
  const ticket = fields(body);
  const spend = fields(ticket["spend"]);
  return {
    repository: text(ticket["repository"]),
    number: count(ticket["number"]),
    title: maybeText(ticket["title"]),
    url: maybeText(ticket["url"]),
    pullRequest: parsePullRequest(ticket["pull_request"]),
    workloads: list(ticket["workloads"]).map((raw) => {
      const run = fields(raw);
      return {
        id: text(run["id"]),
        kind: text(run["kind"]),
        state: text(run["state"]),
        approvedBy: maybeText(run["approved_by"]),
        createdAt: text(run["created_at"]),
        startedAt: maybeText(run["started_at"]),
        finishedAt: maybeText(run["finished_at"]),
        ...(run["lens"] === undefined ? {} : { lens: maybeText(run["lens"]) }),
      };
    }),
    spend: {
      totals: totals(spend["totals"]),
      byKind: Object.fromEntries(
        Object.entries(fields(spend["by_kind"])).map(([kind, value]) => [kind, totals(value)] as const),
      ),
      workloads: count(spend["workloads"]),
    },
  };
}

const NOT_SENT = "The reply could not be sent, and nothing was changed.";

function unsent(error: string): ReplyResult {
  return { commented: false, approved: false, error };
}

// What the API says of a reply, whatever the status: a body that is not its own is "not sent".
async function parseReply(response: Response): Promise<ReplyResult> {
  let body: Fields;
  try {
    body = fields(await response.json());
  } catch {
    return unsent(NOT_SENT);
  }
  const error = typeof body["error"] === "string" ? body["error"] : null;
  const commented = body["commented"] === true;
  const approved = body["approved"] === true;
  if (response.ok) return commented && approved ? { commented, approved, error: null } : unsent(NOT_SENT);
  return { commented, approved, error: error ?? NOT_SENT };
}

function parseQuestions(body: unknown): readonly Question[] {
  return list(fields(body)["questions"]).map((raw) => {
    const row = fields(raw);
    return {
      workload: text(row["workload"]),
      repository: text(row["repository"]),
      ticket: count(row["ticket"]),
      title: maybeText(row["title"]),
      question: text(row["question"]),
      askedAt: maybeText(row["asked_at"]),
    };
  });
}

function parseAgentText(body: unknown): AgentText {
  const text_ = fields(body);
  return { account: maybeText(text_["account"]), question: maybeText(text_["question"]) };
}

function maybeCount(value: unknown): number | null {
  return value === null || value === undefined ? null : count(value);
}

function parseGates(body: unknown): readonly Gate[] {
  return list(fields(body)["gates"]).map((raw) => {
    const gate = fields(raw);
    return {
      name: text(gate["name"]),
      outcome: maybeText(gate["outcome"]),
      reason: maybeText(gate["reason"]),
      ciCheck: maybeText(gate["ci_check"]),
      durationMs: maybeCount(gate["duration_ms"]),
    };
  });
}

// A verdict's requirements are the agent's, and a verdict that lacks a field is still shown.
function parseVerdicts(body: unknown): readonly Verdict[] {
  return list(fields(body)["verdicts"]).map((raw) => {
    const row = fields(raw);
    const verdict = fields(row["verdict"]);
    const requirements = Array.isArray(verdict["requirements"])
      ? (verdict["requirements"] as unknown[])
      : [];
    return {
      workload: text(row["workload"]),
      at: maybeText(row["at"]),
      verdict: text(verdict["verdict"]),
      requirements: requirements.map((item) => {
        const requirement = fields(item);
        const evidence =
          typeof requirement["evidence"] === "object" &&
          requirement["evidence"] !== null
            ? (requirement["evidence"] as Fields)
            : {};
        return {
          status: text(requirement["status"]),
          text:
            typeof requirement["text"] === "string"
              ? requirement["text"]
              : null,
          evidenceKind:
            typeof evidence["kind"] === "string" ? evidence["kind"] : null,
          evidence:
            typeof evidence["detail"] === "string" ? evidence["detail"] : null,
        };
      }),
    };
  });
}

function parseReviewEffort(body: unknown): ReviewEffort {
  const effort = fields(body);
  return {
    localRounds: count(effort["local_rounds"]),
    reviews: count(effort["reviews"]),
    revisions: count(effort["revisions"]),
    lenses: list(effort["lenses"]).map((raw) => {
      const lens = fields(raw);
      return {
        lens: text(lens["lens"]),
        reviews: count(lens["reviews"]),
        found: count(lens["found"]),
        resolved: count(lens["resolved"]),
        remaining: count(lens["remaining"]),
        findings: list(lens["findings"]).map((item) => {
          const finding = fields(item);
          const state = text(finding["state"]);
          if (state !== "resolved" && state !== "remaining") throw shapeError();
          return {
            title: text(finding["title"]),
            severity: maybeText(finding["severity"]),
            blocking: finding["blocking"] === true,
            state,
          };
        }),
      };
    }),
  };
}

function parseDays(body: unknown): readonly SpendDay[] {
  return list(fields(body)["days"]).map((raw) => {
    const day = fields(raw);
    return { day: text(day["day"]), totals: totals(day["totals"]) };
  });
}

function parseDeviceToken(body: unknown): DeviceToken {
  // Only these fields are read: a token's secret is not a field of what a list can show.
  const row = fields(body);
  return {
    id: text(row["id"]),
    name: text(row["name"]),
    createdAt: text(row["createdAt"]),
    lastSeenAt: maybeText(row["lastSeenAt"]),
    revokedAt: maybeText(row["revokedAt"]),
    connected: row["connected"] === true,
    running: count(row["running"]),
    slots: Math.max(1, count(row["slots"])),
  };
}

function parseDeviceTokens(body: unknown): readonly DeviceToken[] {
  return list(fields(body)["tokens"]).map(parseDeviceToken);
}
