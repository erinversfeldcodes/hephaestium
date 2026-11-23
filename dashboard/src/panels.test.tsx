import { render, screen, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ApiError, createClient } from "./api";
import { fakeClient } from "./fixtures";
import type { Follow } from "./stream";
import { RunPage } from "./views/RunPage";
import { TicketDetailPage } from "./views/TicketDetailPage";

const quiet: Follow = () => new Promise(() => undefined);

function ticketPage(overrides = {}) {
  render(
    <TicketDetailPage
      client={fakeClient(overrides)}
      repository="erinversfeldcodes/gazebolang"
      number={12}
    />,
  );
}

describe("a run's gates", () => {
  it("lists each gate with its result and, for a deferred one, the check it was left to", async () => {
    render(<RunPage client={fakeClient()} id="w-2" follow={quiet} />);

    const table = await screen.findByRole("table", { name: "Gates" });
    const rows = within(table).getAllByRole("row").slice(1);
    expect(rows[0]).toHaveTextContent("formatpassed");
    expect(rows[1]).toHaveTextContent(
      "testdeferredtimed out after 60s (left to the check ci / test)",
    );
  });

  it("says when the run kept none, and when they cannot be read", async () => {
    const { unmount } = render(
      <RunPage
        client={fakeClient({ gates: async () => [] })}
        id="w-2"
        follow={quiet}
      />,
    );
    expect(
      await screen.findByText("This run has kept no gates."),
    ).toBeInTheDocument();
    unmount();

    render(
      <RunPage
        client={fakeClient({
          gates: async () => Promise.reject(new ApiError(500, "x")),
        })}
        id="w-2"
        follow={quiet}
      />,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(
      "could not be read",
    );
  });
});

describe("a ticket's verdict history", () => {
  it("lists each verdict, oldest first, with each requirement's evidence", async () => {
    ticketPage();

    const list = await screen.findByRole("list", { name: "Verdicts" });
    const verdicts = within(list)
      .getAllByRole("listitem", { hidden: false })
      .filter((li) => li.querySelector("strong"));
    expect(verdicts).toHaveLength(2);
    expect(verdicts[0]).toHaveTextContent("not_met");
    expect(verdicts[0]).toHaveTextContent(
      "Guards parse (test: no test covers guards)",
    );
    expect(verdicts[1]).toHaveTextContent("met");
  });

  it("says when there is none", async () => {
    ticketPage({ verdicts: async () => [] });
    expect(
      await screen.findByText("No verdict has been given for this ticket."),
    ).toBeInTheDocument();
  });
});

describe("a ticket's review effort", () => {
  it("shows rounds, and for each lens what was found, resolved and still there", async () => {
    ticketPage();

    const table = await screen.findByRole("table", {
      name: "Findings by lens",
    });
    expect(within(table).getAllByRole("row")[1]).toHaveTextContent(
      "security2211",
    );
    expect(
      screen.getByText(/2 rounds before the push, 3 reviews and 1 revisions/),
    ).toBeInTheDocument();
    const findings = screen.getByRole("list", { name: "security findings" });
    expect(findings).toHaveTextContent(
      "leaks a key (blocking, high): resolved",
    );
    expect(findings).toHaveTextContent("naming (advisory, low): remaining");
  });

  it("says when the ticket has not been reviewed", async () => {
    ticketPage({
      reviewEffort: async () => ({
        localRounds: 0,
        reviews: 0,
        revisions: 0,
        lenses: [],
      }),
    });
    expect(
      await screen.findByText("This ticket has not been reviewed."),
    ).toBeInTheDocument();
  });
});

describe("the client's reads of the three panels", () => {
  const respond = (body: unknown): typeof fetch =>
    vi.fn(async () => new Response(JSON.stringify(body)));

  it("reads each from its own route and refuses a body of another shape", async () => {
    const fetcher = respond({
      gates: [{ name: "format", outcome: "passed", duration_ms: 3 }],
    });
    expect(await createClient(fetcher).gates("w 1")).toEqual([
      {
        name: "format",
        outcome: "passed",
        reason: null,
        ciCheck: null,
        durationMs: 3,
      },
    ]);
    expect(fetcher).toHaveBeenCalledWith(
      "/api/workloads/w%201/gates",
      expect.anything(),
    );

    await expect(
      createClient(respond({ gates: [{ outcome: "passed" }] })).gates("w"),
    ).rejects.toBeInstanceOf(ApiError);
    await expect(
      createClient(respond({})).verdicts("o/r", 1),
    ).rejects.toBeInstanceOf(ApiError);
    await expect(
      createClient(
        respond({
          local_rounds: 0,
          reviews: 0,
          revisions: 0,
          lenses: [{ lens: "x" }],
        }),
      ).reviewEffort("o/r", 1),
    ).rejects.toBeInstanceOf(ApiError);
  });

  it("reads a verdict whose requirements carry evidence", async () => {
    const body = {
      verdicts: [
        {
          workload: "w",
          at: null,
          verdict: {
            verdict: "met",
            requirements: [
              { status: "met", evidence: { kind: "test", detail: "d" } },
            ],
          },
        },
      ],
    };
    const [verdict] = await createClient(respond(body)).verdicts("o/r", 1);
    expect(verdict?.requirements[0]).toMatchObject({
      status: "met",
      evidenceKind: "test",
      evidence: "d",
    });
  });
});
