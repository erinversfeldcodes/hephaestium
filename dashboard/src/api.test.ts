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
});
