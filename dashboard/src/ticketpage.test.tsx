import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { PullRequest, Run } from "./api";
import { detail, fakeClient } from "./fixtures";
import { TicketDetailPage } from "./views/TicketDetailPage";

const T0 = Date.parse("2026-11-19T09:00:00Z");
const at = (minutes: number) => new Date(T0 + minutes * 60_000).toISOString();
const now = T0 + 5 * 3_600_000;
const pull = (state: PullRequest["state"], checks: PullRequest["checks"] = "passing"): PullRequest => ({
  number: 63,
  url: "https://github.com/o/r/pull/63",
  state,
  checks,
});
function run(id: string, kind: string, state: string, from: number, to: number | null): Run {
  return { id, kind, state, approvedBy: "octocat", createdAt: at(from), startedAt: at(from), finishedAt: to === null ? null : at(to) };
}

function page(overrides: Parameters<typeof fakeClient>[0] = {}) {
  render(<TicketDetailPage client={fakeClient(overrides)} repository="o/r" number={12} now={now} />);
}

describe("the ticket page's header", () => {
  it("names the repository, number, approver and age, and links the issue and the pull request with its checks", async () => {
    page({ ticket: async () => ({ ...detail, pullRequest: pull("open") }) });

    expect(await screen.findByRole("heading", { name: "Teach the parser about guards" })).toBeInTheDocument();
    expect(screen.getByText(/erinversfeldcodes\/gazebolang #12 · approved by octocat · 5 h ago/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /On GitHub/ })).toHaveAttribute("href", detail.url);
    const pr = screen.getByRole("link", { name: /PR #63 · checks passing/ });
    expect(pr).toHaveAttribute("href", "https://github.com/o/r/pull/63");
    expect(within(pr).getByRole("img")).toHaveAttribute("data-checks", "passing");
  });

  it("has no pull request link when there is no pull request", async () => {
    page();
    await screen.findByRole("link", { name: /On GitHub/ });
    expect(screen.queryByRole("link", { name: /PR #/ })).toBeNull();
  });
});

describe("the four figures", () => {
  it("shows spend, runs and how many run, review rounds with findings and fixes, and elapsed time", async () => {
    page();
    const figures = await screen.findByLabelText("Figures");
    const tile = (name: string) => within(figures).getByText(name).parentElement!;

    expect(tile("Spend")).toHaveTextContent("$1.5012,345 tokens");
    expect(tile("Runs")).toHaveTextContent("21 running");
    await within(tile("Review rounds")).findByText("2 findings, 1 fixed");
    expect(tile("Review rounds")).toHaveTextContent("4"); // 2 before the push, then the first review round and 1 revision
    expect(tile("Elapsed")).toHaveTextContent("5 h");
  });
});

describe("the runs panel", () => {
  const runs = [
    run("a", "implementation", "completed", 0, 40),
    run("b", "review", "failed", 41, 46),
    run("c", "revision", "running", 46, null),
  ];

  it("has a lane for each run, its colour for its state, and each ends in the run's own words", async () => {
    page({ ticket: async () => ({ ...detail, workloads: runs }) });
    const panel = await screen.findByRole("region", { name: "Runs" });
    const lanes = within(panel).getAllByRole("listitem");

    expect(lanes).toHaveLength(3);
    expect(lanes[0]!.querySelector(".bar")).toHaveAttribute("data-tone", "ok");
    expect(lanes[1]!.querySelector(".bar")).toHaveAttribute("data-tone", "bad");
    expect(lanes[2]!.querySelector(".bar")).toHaveAttribute("data-tone", "running");
    expect(lanes[0]).toHaveTextContent("40 min · finished");
    expect(lanes[1]).toHaveTextContent("5 min · failed");
    expect(lanes[2]).toHaveTextContent("running");
    expect(within(panel).getByText("A run is one attempt by an agent. The ticket's stage is above.")).toBeInTheDocument();
    expect(within(lanes[0]!).getByTitle(/The run completed/)).toBeInTheDocument();
  });

  it("puts the bars where the runs' times put them", async () => {
    page({ ticket: async () => ({ ...detail, workloads: [run("a", "implementation", "completed", 0, 40), run("b", "review", "completed", 40, 60)] }) });
    const lanes = within(await screen.findByRole("region", { name: "Runs" })).getAllByRole("listitem");
    const style = (i: number) => (lanes[i]!.querySelector(".bar") as HTMLElement).style;

    expect(parseFloat(style(0).left)).toBeCloseTo(0);
    expect(parseFloat(style(0).width)).toBeCloseTo(66.67, 1);
    expect(parseFloat(style(1).left)).toBeCloseTo(66.67, 1);
    expect(parseFloat(style(1).width)).toBeCloseTo(33.33, 1);
  });

  it("marks a long wait as a break", async () => {
    page({
      ticket: async () => ({
        ...detail,
        pullRequest: pull("merged"),
        workloads: [run("a", "implementation", "completed", 0, 40), run("b", "completeness", "completed", 295, 298)],
      }),
    });
    expect(await screen.findByText("4 h 15 m later, after the merge")).toBeInTheDocument();
  });
});

describe("words for a run and for a ticket", () => {
  const mid = { ...detail, pullRequest: pull("open"), workloads: [run("a", "implementation", "completed", 0, 40), run("b", "revision", "running", 50, null)] };

  it("shows a completed run as finished and never as done", async () => {
    page({ ticket: async () => ({ ...detail, workloads: [run("a", "implementation", "completed", 0, 40)] }), verdicts: async () => [] });
    const panel = await screen.findByRole("region", { name: "Runs" });
    expect(within(panel).getAllByText(/finished/).length).toBeGreaterThan(0);
    expect(within(panel).queryByText(/\bdone\b/i)).toBeNull();
    expect(screen.queryByText(/\bdone\b/i)).toBeNull();
  });

  it("does not show a ticket mid-revision as done, though its implementation run finished", async () => {
    page({ ticket: async () => mid, verdicts: async () => [] });
    const where = await screen.findByRole("region", { name: "Where it is" });
    expect(where).toHaveTextContent("Revising");
    expect(screen.queryByText(/\bdone\b/i)).toBeNull();
  });

  it("says done only for a merged and judged ticket, from the track", async () => {
    page({
      ticket: async () => ({ ...detail, pullRequest: pull("merged"), workloads: [run("a", "implementation", "completed", 0, 40), run("g", "completeness", "completed", 60, 63)] }),
    });
    const where = await screen.findByRole("region", { name: "Where it is" });
    expect(await within(where).findByText("Done")).toBeInTheDocument();
  });
});
