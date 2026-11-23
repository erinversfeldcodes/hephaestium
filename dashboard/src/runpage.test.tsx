import { act, render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ApiError } from "./api";
import { fakeClient, runDetail } from "./fixtures";
import type { Ending, Follow, RunEvent } from "./stream";
import { RunPage } from "./views/RunPage";

function event(sequence: number, kind: string, body: Record<string, unknown> = {}): RunEvent {
  return { sequence, time: "2026-11-20T10:00:00Z", kind, body };
}

/** A stream the test drives: it gives events when told, and ends when told. */
function driven() {
  let emit: (e: RunEvent) => void = () => undefined;
  let end: (e: Ending) => void = () => undefined;
  let signal: AbortSignal | undefined;
  const follow: Follow = (options) => {
    emit = options.onEvent;
    signal = options.signal;
    return new Promise<Ending>((resolve) => {
      end = resolve;
      options.signal.addEventListener("abort", () => resolve("stopped"));
    });
  };
  return {
    follow,
    emit: (e: RunEvent) => act(async () => emit(e)),
    end: (e: Ending) => act(async () => end(e)),
    aborted: () => signal?.aborted ?? false,
  };
}

describe("a run's page", () => {
  it("shows the run's kind and state, and the ticket it belongs to", async () => {
    render(<RunPage client={fakeClient()} id="w-2" follow={driven().follow} />);

    expect(await screen.findByRole("heading", { name: /review/ })).toBeInTheDocument();
    expect(screen.getByText("running")).toHaveAttribute("data-state", "running");
    expect(screen.getByRole("link", { name: /erinversfeldcodes\/gazebolang #12/ })).toHaveAttribute(
      "href",
      "/tickets/erinversfeldcodes/gazebolang/12",
    );
  });

  it("says there is no such run, and when it cannot be read", async () => {
    const { unmount } = render(
      <RunPage client={fakeClient({ workload: async () => null })} id="x" follow={driven().follow} />,
    );
    expect(await screen.findByText("There is no such run.")).toBeInTheDocument();
    unmount();

    render(
      <RunPage
        client={fakeClient({ workload: async () => Promise.reject(new ApiError(500, "x")) })}
        id="x"
        follow={driven().follow}
      />,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("could not be read");
  });

  it("adds each event to the transcript as it arrives, in order, as text", async () => {
    const run = driven();
    render(<RunPage client={fakeClient()} id="w-2" follow={run.follow} />);
    await screen.findByRole("heading", { name: /review/ });

    await run.emit(event(1, "started", { executorVersion: "0.13.0" }));
    await run.emit(event(2, "agentMessage", { text: "<b>I will read</b> the spec" }));
    await run.emit(event(3, "toolCall", { id: "t", name: "Read", input: { file_path: "secret.txt" } }));

    const items = within(screen.getByRole("list", { name: "Events" })).getAllByRole("listitem");
    expect(items.map((item) => item.textContent)).toEqual([
      expect.stringContaining("Executor 0.13.0 started"),
      expect.stringContaining("<b>I will read</b> the spec"),
      expect.stringContaining("Called Read"),
    ]);
    expect(screen.queryByText("secret.txt")).not.toBeInTheDocument();
    expect(document.querySelector("b")).toBeNull();
  });

  it("keeps what a tool returned, a run's reason and its question off the page", async () => {
    const run = driven();
    render(<RunPage client={fakeClient()} id="w-2" follow={run.follow} />);
    await screen.findByRole("heading", { name: /review/ });

    await run.emit(event(4, "toolResult", { callId: "t", output: "FILE CONTENTS", isError: false }));
    await run.emit(
      event(5, "finished", { outcome: "OUTCOME_BLOCKED", reason: "THE REASON", question: "THE QUESTION" }),
    );

    expect(screen.getByText(/Finished: blocked/)).toBeInTheDocument();
    for (const text of ["FILE CONTENTS", "THE REASON", "THE QUESTION"]) {
      expect(screen.queryByText(new RegExp(text))).not.toBeInTheDocument();
    }
  });

  it("does not list a heartbeat", async () => {
    const run = driven();
    render(<RunPage client={fakeClient()} id="w-2" follow={run.follow} />);
    await screen.findByRole("heading", { name: /review/ });

    await run.emit(event(1, "heartbeat"));

    expect(screen.queryByRole("listitem")).not.toBeInTheDocument();
  });

  it("lists each agent's step with its round and lens, and its verdict once it has ended", async () => {
    const run = driven();
    render(<RunPage client={fakeClient()} id="w-2" follow={run.follow} />);
    await screen.findByRole("heading", { name: /review/ });

    await run.emit(event(1, "stepStarted", { kind: "KIND_IMPLEMENTATION" }));
    await run.emit(event(2, "stepStarted", { kind: "KIND_REVIEW", round: 1, lens: "rust" }));

    const steps = within(screen.getByRole("list", { name: "Steps" }));
    expect(steps.getByText("implementation")).toBeInTheDocument();
    expect(steps.getByText("review, round 1, rust")).toBeInTheDocument();
    expect(steps.getAllByText("running")).toHaveLength(2);

    await run.emit(
      event(3, "stepFinished", { kind: "KIND_REVIEW", round: 1, lens: "rust", verdict: "needs_revision" }),
    );

    const review = steps.getByText("review, round 1, rust").closest("li");
    expect(review).toHaveTextContent("needs_revision");
    expect(steps.getAllByText("running")).toHaveLength(1);
  });

  it("says what the stream is doing: following, then ended", async () => {
    const run = driven();
    render(<RunPage client={fakeClient()} id="w-2" follow={run.follow} />);

    expect(await screen.findByRole("status")).toHaveTextContent("Following");

    await run.end("finished");
    expect(screen.getByRole("status")).toHaveTextContent("The run has ended.");
  });

  it("says when the stream was lost, and when the session has ended", async () => {
    const lost = driven();
    const { unmount } = render(<RunPage client={fakeClient()} id="w-2" follow={lost.follow} />);
    await screen.findByRole("heading", { name: /review/ });
    await lost.end("gave-up");
    expect(screen.getByRole("status")).toHaveTextContent("lost");
    unmount();

    const out = driven();
    render(<RunPage client={fakeClient()} id="w-2" follow={out.follow} />);
    await screen.findByRole("heading", { name: /review/ });
    await out.end("signed-out");
    expect(screen.getByRole("link", { name: "Sign in with GitHub" })).toHaveAttribute("href", "/auth/login");
  });

  it("reads the run again when its state moves, so the state shown is the API's", async () => {
    const run = driven();
    let reads = 0;
    const client = fakeClient({
      workload: async () => ({ ...runDetail, state: reads++ === 0 ? "running" : "completed" }),
    });
    render(<RunPage client={client} id="w-2" follow={run.follow} />);
    expect(await screen.findByText("running")).toBeInTheDocument();

    await run.emit(event(9, "finished", { outcome: "OUTCOME_COMPLETED" }));

    expect(await screen.findByText("completed")).toBeInTheDocument();
  });

  it("stops following when the page is left", async () => {
    const run = driven();
    const { unmount } = render(<RunPage client={fakeClient()} id="w-2" follow={run.follow} />);
    await screen.findByRole("heading", { name: /review/ });

    unmount();

    expect(run.aborted()).toBe(true);
  });

  it("goes back to following once a lost connection is back and gives an event", async () => {
    const run = driven();
    let reconnecting: (attempt: number) => void = () => undefined;
    const follow: Follow = (options) => {
      reconnecting = options.onReconnecting ?? reconnecting;
      return run.follow(options);
    };
    render(<RunPage client={fakeClient()} id="w-2" follow={follow} />);
    await screen.findByRole("heading", { name: /review/ });

    await act(async () => reconnecting(2));
    expect(screen.getByRole("status")).toHaveTextContent("trying again (2)");

    await run.emit(event(1, "workspaceReady"));
    expect(screen.getByRole("status")).toHaveTextContent("Following the run.");
  });
});
