import { describe, expect, it, vi } from "vitest";
import { ApiError, createClient } from "./api";

function respond(status: number, body: unknown): typeof fetch {
  return vi.fn(async () => new Response(JSON.stringify(body), { status }));
}

describe("the API client", () => {
  it("reads who is signed in", async () => {
    const client = createClient(respond(200, { login: "octocat" }));

    expect(await client.session()).toEqual({ login: "octocat" });
  });

  it("takes a 401 to mean nobody is signed in, not a failure", async () => {
    const client = createClient(respond(401, { error: "not signed in" }));

    expect(await client.session()).toBeNull();
  });

  it("raises for any other failure, naming the status", async () => {
    const client = createClient(respond(500, { error: "boom" }));

    await expect(client.session()).rejects.toMatchObject({ status: 500 });
    await expect(client.session()).rejects.toBeInstanceOf(ApiError);
  });

  it("raises when the answer is not the shape the contract promises", async () => {
    for (const body of [{}, { login: 7 }, null, "octocat"]) {
      const client = createClient(respond(200, body));

      await expect(client.session()).rejects.toBeInstanceOf(ApiError);
    }
  });

  it("passes on a network failure", async () => {
    const client = createClient(vi.fn(async () => Promise.reject(new TypeError("offline"))));

    await expect(client.session()).rejects.toThrow("offline");
  });

  it("only ever reads: every request is a GET that sends the session cookie and nothing else", async () => {
    const fetcher = respond(200, { login: "octocat" });

    await createClient(fetcher).session();

    const calls = vi.mocked(fetcher).mock.calls;
    expect(calls).toHaveLength(1);
    const [url, init] = calls[0] ?? [];
    expect(url).toBe("/api/session");
    expect(init?.method ?? "GET").toBe("GET");
    expect(init?.body).toBeUndefined();
    expect(init?.credentials).toBe("same-origin");
  });

  describe("a question", () => {
    const row = {
      workload: "w-3",
      repository: "o/r",
      ticket: 12,
      title: "T",
      question: "Which database?",
      asked_at: "2026-11-20T11:00:00Z",
    };

    it("is the one waiting for this ticket, read from the list of questions", async () => {
      const fetcher = respond(200, { questions: [{ ...row, ticket: 7 }, row] });

      expect(await createClient(fetcher).question("o/r", 12)).toEqual({
        workload: "w-3",
        repository: "o/r",
        ticket: 12,
        title: "T",
        question: "Which database?",
        askedAt: "2026-11-20T11:00:00Z",
      });
      expect(vi.mocked(fetcher).mock.calls[0]?.[0]).toBe("/api/questions");
    });

    it("is null when none waits for the ticket, and not mixed up with another repository's", async () => {
      const fetcher = respond(200, { questions: [{ ...row, repository: "o/other" }] });

      expect(await createClient(fetcher).question("o/r", 12)).toBeNull();
    });

    it("lists every waiting question, with GET only", async () => {
      const fetcher = respond(200, { questions: [row, { ...row, ticket: 7 }] });

      const all = await createClient(fetcher).questions();

      expect(all.map((q) => q.ticket)).toEqual([12, 7]);
      expect(vi.mocked(fetcher).mock.calls[0]?.[1]?.method).toBe("GET");
    });

    it("raises on a row that is not the shape promised", async () => {
      await expect(
        createClient(respond(200, { questions: [{ ...row, question: 3 }] })).question("o/r", 12),
      ).rejects.toBeInstanceOf(ApiError);
    });
  });

  describe("a reply", () => {
    function exchange(post: Response): typeof fetch {
      return vi.fn(async (input: RequestInfo | URL, init?: RequestInit) =>
        String(input) === "/api/csrf" && (init?.method ?? "GET") === "GET"
          ? new Response(JSON.stringify({ csrfToken: "the-csrf-token" }), { status: 200 })
          : post,
      );
    }

    it("asks for a CSRF token and then posts the reply with it, to the ticket's own address and nowhere else", async () => {
      const fetcher = exchange(new Response(JSON.stringify({ commented: true, approved: true }), { status: 200 }));

      const result = await createClient(fetcher).reply("o/r", 12, "Use Postgres.");

      expect(result).toEqual({ commented: true, approved: true, error: null });
      const calls = vi.mocked(fetcher).mock.calls;
      expect(calls.map(([url, init]) => `${init?.method ?? "GET"} ${String(url)}`)).toEqual([
        "GET /api/csrf",
        "POST /api/tickets/o/r/12/reply",
      ]);
      const [, init] = calls[1] ?? [];
      expect(init?.credentials).toBe("same-origin");
      expect(new Headers(init?.headers).get("x-csrf-token")).toBe("the-csrf-token");
      expect(new Headers(init?.headers).get("content-type")).toBe("application/json");
      expect(JSON.parse(String(init?.body))).toEqual({ body: "Use Postgres." });
    });

    it("says what was done when the API refuses part of it, in the API's own words", async () => {
      const refused = new Response(
        JSON.stringify({ error: "GitHub refused the approval", commented: true, approved: false }),
        { status: 403 },
      );

      expect(await createClient(exchange(refused)).reply("o/r", 12, "x")).toEqual({
        commented: true,
        approved: false,
        error: "GitHub refused the approval",
      });
    });

    it("says nothing was sent when the answer is not one it understands", async () => {
      const result = await createClient(exchange(new Response("<html>", { status: 502 }))).reply("o/r", 12, "x");

      expect(result).toEqual({
        commented: false,
        approved: false,
        error: "The reply could not be sent, and nothing was changed.",
      });
    });

    it("sends nothing when no CSRF token could be had", async () => {
      const fetcher = vi.fn(async () => new Response("{}", { status: 401 }));

      const result = await createClient(fetcher).reply("o/r", 12, "x");

      expect(result).toMatchObject({ commented: false, approved: false });
      expect(result.error).toContain("sign in");
      expect(vi.mocked(fetcher)).toHaveBeenCalledTimes(1);
    });

    it("makes no other write: every call but that one POST is a GET", async () => {
      const fetcher = exchange(new Response(JSON.stringify({ commented: true, approved: true }), { status: 200 }));
      const client = createClient(fetcher);

      await client.reply("o/r", 12, "x");
      await client.question("o/r", 12).catch(() => null);
      await client.tickets().catch(() => null);

      const methods = vi.mocked(fetcher).mock.calls.map(([, init]) => init?.method ?? "GET");
      expect(methods.filter((method) => method !== "GET")).toEqual(["POST"]);
    });
  });

  describe("tickets", () => {
    const row = {
      repository: "o/r",
      number: 3,
      title: "A title",
      workloads: 2,
      settled_at: "2026-11-20T11:00:00Z",
      latest: { id: "w", state: "succeeded", kind: "revision", at: "2026-11-20T10:00:00Z" },
    };

    it("reads the list, in camel case, and asks only for the list", async () => {
      const fetcher = respond(200, { tickets: [row, { ...row, number: 4, title: null, settled_at: null }] });

      const tickets = await createClient(fetcher).tickets();

      expect(tickets).toHaveLength(2);
      expect(tickets[0]).toEqual({
        repository: "o/r",
        number: 3,
        title: "A title",
        workloads: 2,
        settledAt: "2026-11-20T11:00:00Z",
        latest: { id: "w", state: "succeeded", kind: "revision", at: "2026-11-20T10:00:00Z" },
      });
      expect(tickets[1]).toMatchObject({ title: null, settledAt: null });
      expect(vi.mocked(fetcher).mock.calls[0]?.[0]).toBe("/api/tickets?limit=200");
    });

    it("raises on a row that is not the shape promised", async () => {
      await expect(createClient(respond(200, { tickets: [{ ...row, number: "3" }] })).tickets()).rejects.toBeInstanceOf(ApiError);
      await expect(createClient(respond(200, { tickets: "none" })).tickets()).rejects.toBeInstanceOf(ApiError);
    });

    it("raises on 401 so the page can offer the sign-in", async () => {
      await expect(createClient(respond(401, {})).tickets()).rejects.toMatchObject({ status: 401 });
    });
  });

  describe("a ticket", () => {
    const body = {
      repository: "o/r",
      number: 3,
      title: "A title",
      url: "https://github.com/o/r/issues/3",
      workloads: [
        {
          id: "w",
          repository: "o/r",
          ticket_number: 3,
          kind: "implementation",
          state: "blocked",
          reason: "something an agent said",
          parent_id: null,
          approved_by: "octocat",
          started_at: "2026-11-20T10:00:00Z",
          finished_at: null,
          settled_at: null,
          created_at: "2026-11-20T09:00:00Z",
        },
      ],
      spend: { totals: { usd: 2 }, by_kind: { implementation: { usd: 2 } }, workloads: 1, parts: null },
    };

    it("reads the ticket, its runs and its spend, and leaves the run's reason unread", async () => {
      const fetcher = respond(200, body);

      const ticket = await createClient(fetcher).ticket("o/r", 3);

      expect(ticket).toEqual({
        repository: "o/r",
        number: 3,
        title: "A title",
        url: "https://github.com/o/r/issues/3",
        workloads: [
          {
            id: "w",
            kind: "implementation",
            state: "blocked",
            approvedBy: "octocat",
            createdAt: "2026-11-20T09:00:00Z",
            startedAt: "2026-11-20T10:00:00Z",
            finishedAt: null,
          },
        ],
        spend: { totals: { usd: 2 }, byKind: { implementation: { usd: 2 } }, workloads: 1 },
      });
      expect(JSON.stringify(ticket)).not.toContain("something an agent said");
      expect(vi.mocked(fetcher).mock.calls[0]?.[0]).toBe("/api/tickets/o/r/3");
    });

    it("is null when the API has no such ticket", async () => {
      expect(await createClient(respond(404, { error: "not found" })).ticket("o/r", 3)).toBeNull();
    });

    it("raises on another failure or a shape not promised", async () => {
      await expect(createClient(respond(500, {})).ticket("o/r", 3)).rejects.toMatchObject({ status: 500 });
      await expect(createClient(respond(200, { ...body, workloads: [{}] })).ticket("o/r", 3)).rejects.toBeInstanceOf(ApiError);
    });
  });

  describe("spend by day", () => {
    it("reads the days for a repository, with the repository and the days in the request", async () => {
      const fetcher = respond(200, { days: [{ day: "2026-11-20", totals: { usd: 1, tokens: 5 } }] });

      expect(await createClient(fetcher).spendDays("o/r", 14)).toEqual([
        { day: "2026-11-20", totals: { usd: 1, tokens: 5 } },
      ]);
      expect(vi.mocked(fetcher).mock.calls[0]?.[0]).toBe("/api/spend/days?repository=o%2Fr&days=14");
    });

    it("raises on a total that is not a number", async () => {
      const fetcher = respond(200, { days: [{ day: "2026-11-20", totals: { usd: "1" } }] });

      await expect(createClient(fetcher).spendDays("o/r", 14)).rejects.toBeInstanceOf(ApiError);
    });
  });
});
