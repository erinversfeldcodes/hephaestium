// The dashboard reads the control plane's API, and writes one thing: a person's reply to an
// agent's question. Every request is a GET (through `get`) except the reply's POST, which is
// made in `reply` alone, to the one address a ticket's reply has, with the CSRF token the API
// gave for the session. A person's session is the cookie the browser already holds.
//
// What the API answers is checked as it is read: a body that is not the shape promised is an
// error, never a half-built value for a page to trip over. A workload's `reason` is not read
// at all, since it can carry what an agent said, which the dashboard does not show yet.

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

export interface TicketSummary {
  readonly repository: string;
  readonly number: number;
  readonly title: string | null;
  readonly workloads: number;
  readonly settledAt: string | null;
  readonly latest: {
    readonly id: string;
    readonly state: string;
    readonly kind: string;
    readonly at: string;
  };
}

export interface Run {
  readonly id: string;
  readonly kind: string;
  readonly state: string;
  readonly approvedBy: string | null;
  readonly createdAt: string;
  readonly startedAt: string | null;
  readonly finishedAt: string | null;
}

export interface TicketDetail {
  readonly repository: string;
  readonly number: number;
  readonly title: string | null;
  readonly url: string | null;
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

export interface Client {
  /** Who is signed in, or null when nobody is. Any other failure raises. */
  session(): Promise<Session | null>;
  /** The tickets that have had runs, the most recently active first. */
  tickets(): Promise<readonly TicketSummary[]>;
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
  };

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

function parseTickets(body: unknown): readonly TicketSummary[] {
  return list(fields(body)["tickets"]).map((raw) => {
    const row = fields(raw);
    const latest = fields(row["latest"]);
    return {
      repository: text(row["repository"]),
      number: count(row["number"]),
      title: maybeText(row["title"]),
      workloads: count(row["workloads"]),
      settledAt: maybeText(row["settled_at"]),
      latest: {
        id: text(latest["id"]),
        state: text(latest["state"]),
        kind: text(latest["kind"]),
        at: text(latest["at"]),
      },
    };
  });
}

function parseTicket(body: unknown): TicketDetail {
  const ticket = fields(body);
  const spend = fields(ticket["spend"]);
  return {
    repository: text(ticket["repository"]),
    number: count(ticket["number"]),
    title: maybeText(ticket["title"]),
    url: maybeText(ticket["url"]),
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

function parseDays(body: unknown): readonly SpendDay[] {
  return list(fields(body)["days"]).map((raw) => {
    const day = fields(raw);
    return { day: text(day["day"]), totals: totals(day["totals"]) };
  });
}
