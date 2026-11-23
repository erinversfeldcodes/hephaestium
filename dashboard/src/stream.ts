// A run's events as they are kept, read from the API's server-sent events with `fetch` rather
// than `EventSource`: the browser's own reader reconnects for ever and at its own pace, cannot
// be told to stop after a number of tries, and cannot be handed a fake in a test. This one
// resumes from the last event it had, waits longer after each failure up to a limit, gives up
// after a number of tries that gave nothing, and stops at the event that finishes the run.

export interface RunEvent {
  readonly sequence: number;
  readonly time: string;
  readonly kind: string;
  /** What the event says, the object under its kind. */
  readonly body: Readonly<Record<string, unknown>>;
}

/** How a stream ended: the run finished, the reader stopped, the session ended, or it was lost. */
export type Ending = "finished" | "stopped" | "signed-out" | "gave-up";

export interface FollowOptions {
  readonly workload: string;
  readonly signal: AbortSignal;
  readonly onEvent: (event: RunEvent) => void;
  /** Told when a connection is lost and another will be tried, with how many have been. */
  readonly onReconnecting?: (attempt: number) => void;
  readonly fetcher?: typeof fetch;
  /** Waits before the next try; it returns early when the signal aborts. */
  readonly wait?: (ms: number, signal: AbortSignal) => Promise<void>;
  readonly maxAttempts?: number;
}

export type Follow = (options: FollowOptions) => Promise<Ending>;

export interface Frame {
  readonly id: string | null;
  readonly event: string | null;
  readonly data: string;
}

const BASE_MS = 500;
const CAP_MS = 8000;
const MAX_ATTEMPTS = 6;

/** The frames of a server-sent stream. A comment (a keepalive) is not one; an unfinished frame is dropped. */
export async function* readFrames(source: ReadableStream<Uint8Array>): AsyncGenerator<Frame> {
  const reader = source.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) return;
      buffer += decoder.decode(value, { stream: true }).replace(/\r\n?/g, "\n");
      for (let end = buffer.indexOf("\n\n"); end !== -1; end = buffer.indexOf("\n\n")) {
        const frame = parseFrame(buffer.slice(0, end));
        buffer = buffer.slice(end + 2);
        if (frame !== null) yield frame;
      }
    }
  } finally {
    await reader.cancel().catch(() => undefined);
  }
}

function parseFrame(block: string): Frame | null {
  let id: string | null = null;
  let event: string | null = null;
  const data: string[] = [];
  for (const line of block.split("\n")) {
    if (line === "" || line.startsWith(":")) continue;
    const colon = line.indexOf(":");
    const field = colon === -1 ? line : line.slice(0, colon);
    const value = colon === -1 ? "" : line.slice(colon + 1).replace(/^ /, "");
    if (field === "id") id = value;
    else if (field === "event") event = value;
    else if (field === "data") data.push(value);
  }
  return data.length === 0 ? null : { id, event, data: data.join("\n") };
}

/** An event as the API writes it, or null when it is not that. */
function parseEvent(data: string): RunEvent | null {
  let value: unknown;
  try {
    value = JSON.parse(data);
  } catch {
    return null;
  }
  if (typeof value !== "object" || value === null) return null;
  const row = value as Record<string, unknown>;
  const { sequence, time, kind, event } = row;
  if (typeof sequence !== "number" || !Number.isSafeInteger(sequence) || sequence < 1) return null;
  if (typeof time !== "string" || typeof kind !== "string") return null;
  const inner = typeof event === "object" && event !== null ? (event as Record<string, unknown>)[kind] : undefined;
  const body = typeof inner === "object" && inner !== null && !Array.isArray(inner) ? (inner as Record<string, unknown>) : {};
  return { sequence, time, kind, body };
}

function pause(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    if (signal.aborted) return resolve();
    const timer = setTimeout(done, ms);
    function done() {
      clearTimeout(timer);
      signal.removeEventListener("abort", done);
      resolve();
    }
    signal.addEventListener("abort", done);
  });
}

type Attempt = "finished" | "ended" | "signed-out" | "missing";

export async function followEvents(options: FollowOptions): Promise<Ending> {
  const fetcher = options.fetcher ?? ((...args: Parameters<typeof fetch>) => fetch(...args));
  const wait = options.wait ?? pause;
  const maxAttempts = options.maxAttempts ?? MAX_ATTEMPTS;
  const { signal } = options;
  let last = 0;
  let failures = 0;

  async function attempt(): Promise<Attempt> {
    const headers: Record<string, string> = last > 0 ? { "last-event-id": String(last) } : {};
    const response = await fetcher(`/api/workloads/${encodeURIComponent(options.workload)}/events/stream`, {
      method: "GET",
      credentials: "same-origin",
      headers,
      signal,
    });
    if (response.status === 401) return "signed-out";
    if (response.status === 404) return "missing";
    if (!response.ok || response.body === null) throw new Error(`the stream answered ${response.status}`);

    for await (const frame of readFrames(response.body)) {
      const event = parseEvent(frame.data);
      if (event === null) throw new Error("an event is not the shape the API promises");
      if (event.sequence <= last) continue;
      last = event.sequence;
      failures = 0;
      options.onEvent(event);
      if (signal.aborted) return "ended";
      if (event.kind === "finished") return "finished";
    }
    return "ended";
  }

  while (!signal.aborted) {
    let result: Attempt = "ended";
    try {
      result = await attempt();
    } catch {
      result = "ended";
    }
    if (signal.aborted) break;
    if (result === "finished") return "finished";
    if (result === "signed-out") return "signed-out";
    if (result === "missing") return "gave-up";

    if (failures >= maxAttempts) return "gave-up";
    options.onReconnecting?.(failures + 1);
    const delay = Math.min(CAP_MS, BASE_MS * 2 ** failures);
    failures += 1;
    await wait(delay, signal);
  }
  return "stopped";
}
