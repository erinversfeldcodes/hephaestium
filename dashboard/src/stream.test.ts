import { describe, expect, it, vi } from "vitest";
import { followEvents, readFrames, type RunEvent } from "./stream";

const encoder = new TextEncoder();

function body(chunks: readonly string[]): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
      controller.close();
    },
  });
}

function frame(sequence: number, kind: string, bodyOf: Record<string, unknown> = {}): string {
  const data = JSON.stringify({ sequence, time: "2026-11-20T10:00:00Z", kind, event: { [kind]: bodyOf } });
  return `id: ${sequence}\nevent: ${kind}\ndata: ${data}\n\n`;
}

function stream(...chunks: string[]): Response {
  return new Response(body(chunks), { status: 200, headers: { "content-type": "text/event-stream" } });
}

/** Answers each request with the next response, and keeps what was asked. */
function sequence(...responses: (Response | Error)[]) {
  const calls: { url: string; init: RequestInit | undefined }[] = [];
  const fetcher = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ url: String(input), init });
    const next = responses.shift();
    if (next === undefined) throw new Error("no more answers");
    if (next instanceof Error) throw next;
    return next;
  });
  return { fetcher: fetcher as unknown as typeof fetch, calls };
}

async function collect(
  run: (onEvent: (event: RunEvent) => void, waits: number[]) => Promise<string>,
): Promise<{ ending: string; kinds: number[]; waits: number[] }> {
  const kinds: number[] = [];
  const waits: number[] = [];
  const ending = await run((event) => kinds.push(event.sequence), waits);
  return { ending, kinds, waits };
}

function follow(fetcher: typeof fetch, options: { signal?: AbortSignal; maxAttempts?: number } = {}) {
  return collect((onEvent, waits) =>
    followEvents({
      workload: "w-2",
      fetcher,
      signal: options.signal ?? new AbortController().signal,
      onEvent,
      wait: async (ms) => {
        waits.push(ms);
      },
      ...(options.maxAttempts === undefined ? {} : { maxAttempts: options.maxAttempts }),
    }),
  );
}

describe("reading server-sent frames", () => {
  async function frames(chunks: string[]) {
    const out = [];
    for await (const f of readFrames(body(chunks))) out.push(f);
    return out;
  }

  it("joins a frame split across chunks, and takes CRLF line ends", async () => {
    expect(await frames(["id: 3\nevent: agentM", "essage\r\ndata: {\"a\":", "1}\r\n\r\n"])).toEqual([
      { id: "3", event: "agentMessage", data: '{"a":1}' },
    ]);
  });

  it("joins data lines with a newline, and skips comments such as a keepalive", async () => {
    expect(await frames([": keepalive\n\n", "data: one\ndata: two\n\n"])).toEqual([
      { id: null, event: null, data: "one\ntwo" },
    ]);
  });

  it("does not give a frame that never ended", async () => {
    expect(await frames(["data: half"])).toEqual([]);
  });
});

describe("following a run's events", () => {
  it("gives each event in order and stops at the one that finishes the run, without asking again", async () => {
    const { fetcher, calls } = sequence(
      stream(frame(1, "started"), frame(2, "agentMessage", { text: "hi" })),
      stream(frame(3, "finished", { outcome: "OUTCOME_COMPLETED" })),
    );

    const result = await follow(fetcher);

    // the first answer ends without the finishing event, so it asks again, from where it was
    expect(result.kinds).toEqual([1, 2, 3]);
    expect(result.ending).toBe("finished");
    expect(calls).toHaveLength(2);
  });

  it("asks only with GET, for the cookie's session, and begins with no cursor", async () => {
    const { fetcher, calls } = sequence(stream(frame(1, "finished", { outcome: "OUTCOME_COMPLETED" })));

    await follow(fetcher);

    expect(calls[0]?.url).toBe("/api/workloads/w-2/events/stream");
    expect(calls[0]?.init?.method).toBe("GET");
    expect(calls[0]?.init?.credentials).toBe("same-origin");
    expect(new Headers(calls[0]?.init?.headers).get("last-event-id")).toBeNull();
  });

  it("resumes after a drop with the Last-Event-ID of the last event it had, and ignores a replay", async () => {
    const { fetcher, calls } = sequence(
      stream(frame(1, "started"), frame(2, "workspaceReady")),
      stream(frame(2, "workspaceReady"), frame(3, "finished", { outcome: "OUTCOME_COMPLETED" })),
    );

    const result = await follow(fetcher);

    expect(new Headers(calls[1]?.init?.headers).get("last-event-id")).toBe("2");
    expect(result.kinds).toEqual([1, 2, 3]);
  });

  it("waits longer after each drop, up to a limit, and gives up after the most attempts it is allowed", async () => {
    const down = () => new Error("network");
    const { fetcher, calls } = sequence(down(), down(), down(), down(), down(), down(), down());

    const result = await follow(fetcher, { maxAttempts: 6 });

    expect(result.ending).toBe("gave-up");
    expect(calls).toHaveLength(7);
    expect(result.waits).toEqual([500, 1000, 2000, 4000, 8000, 8000]);
  });

  it("starts counting again once a connection has given an event", async () => {
    const down = () => new Error("network");
    const { fetcher } = sequence(
      down(),
      down(),
      stream(frame(1, "started")),
      down(),
      stream(frame(2, "finished", { outcome: "OUTCOME_COMPLETED" })),
    );

    const result = await follow(fetcher, { maxAttempts: 3 });

    expect(result.ending).toBe("finished");
    expect(result.waits).toEqual([500, 1000, 500, 1000]);
  });

  it("takes a refusal of the session as the end, with no retry, and a missing run likewise", async () => {
    for (const [status, ending] of [
      [401, "signed-out"],
      [404, "gave-up"],
    ] as const) {
      const { fetcher, calls } = sequence(new Response("", { status }));

      const result = await follow(fetcher);

      expect(result.ending).toBe(ending);
      expect(calls).toHaveLength(1);
    }
  });

  it("tries again after a server error", async () => {
    const { fetcher } = sequence(
      new Response("", { status: 502 }),
      stream(frame(1, "finished", { outcome: "OUTCOME_COMPLETED" })),
    );

    expect((await follow(fetcher)).ending).toBe("finished");
  });

  it("does not hand on an event that is not the shape the API promises, and tries again from the same place", async () => {
    const bad = `id: 2\nevent: x\ndata: {"sequence":"two"}\n\n`;
    const { fetcher, calls } = sequence(
      stream(frame(1, "started"), bad),
      stream(frame(2, "finished", { outcome: "OUTCOME_COMPLETED" })),
    );

    const result = await follow(fetcher);

    expect(result.kinds).toEqual([1, 2]);
    expect(new Headers(calls[1]?.init?.headers).get("last-event-id")).toBe("1");
  });

  it("stops when it is told to, and does not ask again", async () => {
    const controller = new AbortController();
    const { fetcher, calls } = sequence(stream(frame(1, "started")), stream(frame(2, "workspaceReady")));
    const kinds: number[] = [];

    const ending = await followEvents({
      workload: "w-2",
      fetcher,
      signal: controller.signal,
      onEvent: (event) => {
        kinds.push(event.sequence);
        controller.abort();
      },
      wait: async () => undefined,
    });

    expect(ending).toBe("stopped");
    expect(kinds).toEqual([1]);
    expect(calls).toHaveLength(1);
  });

  it("stops while it is waiting to try again", async () => {
    const controller = new AbortController();
    const { fetcher, calls } = sequence(new Error("network"), stream(frame(1, "started")));

    const ending = await followEvents({
      workload: "w-2",
      fetcher,
      signal: controller.signal,
      onEvent: () => undefined,
      wait: async () => controller.abort(),
    });

    expect(ending).toBe("stopped");
    expect(calls).toHaveLength(1);
  });
});
