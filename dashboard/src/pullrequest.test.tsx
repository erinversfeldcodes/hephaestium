import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { createClient, type PullRequest } from "./api";
import { detail, fakeClient, summary } from "./fixtures";
import { BoardPage } from "./views/BoardPage";
import { TicketDetailPage } from "./views/TicketDetailPage";

const url = "https://github.com/erinversfeldcodes/gazebolang/pull/52";
const pull = (checks: PullRequest["checks"], state: PullRequest["state"] = "open"): PullRequest => ({
  number: 52,
  url,
  state,
  checks,
});

describe("a ticket's pull request on the board", () => {
  it("is a link to the pull request by its number, beside the ticket", async () => {
    render(<BoardPage client={fakeClient({ tickets: async () => [{ ...summary, pullRequest: pull("passing") }] })} />);

    const link = await screen.findByRole("link", { name: /#52/ });
    expect(link).toHaveAttribute("href", url);
  });

  it.each([
    ["passing", "checks passing"],
    ["failing", "checks failing"],
    ["pending", "checks pending"],
    ["none", "checks none"],
  ] as const)("shows the dot for %s checks", async (checks, label) => {
    render(<BoardPage client={fakeClient({ tickets: async () => [{ ...summary, pullRequest: pull(checks) }] })} />);

    const link = await screen.findByRole("link", { name: /#52/ });
    const dot = within(link).getByRole("img");
    expect(dot).toHaveAttribute("data-checks", checks);
    expect(dot).toHaveAttribute("aria-label", expect.stringContaining(label));
  });

  it("has no link and no placeholder for a ticket with no pull request", async () => {
    render(<BoardPage client={fakeClient({ tickets: async () => [summary] })} />);

    await screen.findByRole("link", { name: "Teach the parser about guards" });
    expect(document.querySelector(".pull-request")).toBeNull();
    expect(document.querySelector(".checks-dot")).toBeNull();
  });
});

describe("a ticket's pull request on the ticket page", () => {
  it("is a link to the pull request with the dot for its checks", async () => {
    render(
      <TicketDetailPage
        client={fakeClient({ ticket: async () => ({ ...detail, pullRequest: pull("failing") }) })}
        repository="erinversfeldcodes/gazebolang"
        number={12}
      />,
    );

    const link = await screen.findByRole("link", { name: /#52/ });
    expect(link).toHaveAttribute("href", url);
    expect(within(link).getByRole("img")).toHaveAttribute("data-checks", "failing");
  });

  it("has no link for a ticket with no pull request", async () => {
    render(<TicketDetailPage client={fakeClient()} repository="erinversfeldcodes/gazebolang" number={12} />);

    await screen.findByRole("heading", { name: /Teach the parser about guards/ });
    expect(document.querySelector(".pull-request")).toBeNull();
  });
});

describe("reading a pull request from the API", () => {
  const respond = (body: unknown) => async () => new Response(JSON.stringify(body), { status: 200 });
  const raw = { number: 52, url, state: "merged", checks: "passing" };

  it("reads it on the board, and null when there is none", async () => {
    const row = { repository: "o/r", number: 3, title: "T", workloads: 1, settled_at: null, latest: null, waiting: null };
    const tickets = await createClient(respond({ tickets: [{ ...row, pull_request: raw }, { ...row, number: 4, pull_request: null }] })).tickets();

    expect(tickets[0]?.pullRequest).toEqual({ number: 52, url, state: "merged", checks: "passing" });
    expect(tickets[1]?.pullRequest).toBeNull();
  });

  it("refuses a state or checks it does not know, rather than draw a wrong dot", async () => {
    const row = { repository: "o/r", number: 3, title: "T", workloads: 1, settled_at: null, latest: null, waiting: null };
    const client = createClient(respond({ tickets: [{ ...row, pull_request: { ...raw, checks: "great" } }] }));

    await expect(client.tickets()).rejects.toThrow();
  });
});
