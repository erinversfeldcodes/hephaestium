import { render, screen, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ApiError } from "./api";
import { days, detail, fakeClient, summary } from "./fixtures";
import { SpendPage } from "./views/SpendPage";
import { TicketDetailPage } from "./views/TicketDetailPage";
import { TicketListPage } from "./views/TicketListPage";

const forever = () => new Promise<never>(() => undefined);

describe("the ticket list", () => {
  it("lists each ticket with its title, its latest run and how many runs it has had", async () => {
    render(<TicketListPage client={fakeClient()} />);

    const row = await screen.findByRole("row", { name: /Teach the parser about guards/ });
    expect(within(row).getByRole("link", { name: "Teach the parser about guards" })).toHaveAttribute(
      "href",
      "/tickets/erinversfeldcodes/gazebolang/12",
    );
    expect(within(row).getByText("erinversfeldcodes/gazebolang #12")).toBeInTheDocument();
    expect(within(row).getByText("review")).toBeInTheDocument();
    expect(within(row).getByText("running")).toBeInTheDocument();
    expect(within(row).getByText("2")).toBeInTheDocument();
  });

  it("names a ticket by its number when it has no title", async () => {
    render(<TicketListPage client={fakeClient({ tickets: async () => [{ ...summary, title: null }] })} />);

    expect(await screen.findByRole("link", { name: "#12" })).toBeInTheDocument();
  });

  it("says when there are no tickets", async () => {
    render(<TicketListPage client={fakeClient({ tickets: async () => [] })} />);

    expect(await screen.findByText("No ticket has had a run yet.")).toBeInTheDocument();
  });

  it("says it is loading, and says so when it fails", async () => {
    const { unmount } = render(<TicketListPage client={fakeClient({ tickets: forever })} />);
    expect(screen.getByText("Loading")).toBeInTheDocument();
    unmount();

    render(<TicketListPage client={fakeClient({ tickets: async () => Promise.reject(new ApiError(500, "x")) })} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("could not be read");
  });

  it("offers the sign-in when the session has ended", async () => {
    render(<TicketListPage client={fakeClient({ tickets: async () => Promise.reject(new ApiError(401, "x")) })} />);

    expect(await screen.findByRole("link", { name: "Sign in with GitHub" })).toHaveAttribute("href", "/auth/login");
  });
});

describe("a ticket", () => {
  it("shows its runs with their state and kind, who approved, and what it spent", async () => {
    render(<TicketDetailPage client={fakeClient()} repository="erinversfeldcodes/gazebolang" number={12} />);

    expect(await screen.findByRole("heading", { name: /Teach the parser about guards/ })).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "On GitHub" })).toHaveAttribute("href", detail.url);

    const runs = screen.getAllByRole("row").slice(1);
    expect(runs).toHaveLength(2);
    expect(within(runs[0]!).getByText("implementation")).toBeInTheDocument();
    expect(within(runs[0]!).getByText("succeeded")).toBeInTheDocument();
    expect(within(runs[0]!).getByText("octocat")).toBeInTheDocument();
    expect(within(runs[1]!).getByText("running")).toBeInTheDocument();

    const spend = screen.getByRole("region", { name: "Spend" });
    expect(within(spend).getByText("$1.50 and 12,345 tokens")).toBeInTheDocument();
    expect(within(spend).getByText("implementation")).toBeInTheDocument();
    expect(within(spend).getByText("$1.25")).toBeInTheDocument();
  });

  it("says when there is no such ticket", async () => {
    render(<TicketDetailPage client={fakeClient({ ticket: async () => null })} repository="o/r" number={1} />);

    expect(await screen.findByText("No run has been made for this ticket.")).toBeInTheDocument();
  });

  it("asks for the ticket it was given", async () => {
    const asked: Array<[string, number]> = [];
    render(
      <TicketDetailPage
        client={fakeClient({
          ticket: async (repository, number) => {
            asked.push([repository, number]);
            return detail;
          },
        })}
        repository="o/r"
        number={9}
      />,
    );

    await screen.findByRole("heading", { level: 2 });
    expect(asked).toEqual([["o/r", 9]]);
  });

  it("shows nothing of what the agent asked or answered", async () => {
    render(<TicketDetailPage client={fakeClient()} repository="erinversfeldcodes/gazebolang" number={12} />);

    await screen.findByRole("heading", { level: 2 });
    expect(screen.queryByText(/question/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/account/i)).not.toBeInTheDocument();
  });
});

describe("spend", () => {
  it("lists each day's spend for the repository chosen", async () => {
    const asked: Array<[string, number]> = [];
    render(
      <SpendPage
        client={fakeClient({
          spendDays: async (repository, count) => {
            asked.push([repository, count]);
            return days;
          },
        })}
        repository="erinversfeldcodes/gazebolang"
      />,
    );

    expect(await screen.findByRole("row", { name: /2026-11-19/ })).toHaveTextContent("$1.25 and 10,345 tokens");
    expect(screen.getByRole("row", { name: /2026-11-20/ })).toHaveTextContent("$0.25 and 2,000 tokens");
    expect(asked).toEqual([["erinversfeldcodes/gazebolang", 14]]);
  });

  it("asks which repository when none is chosen, from those that have tickets", async () => {
    render(<SpendPage client={fakeClient()} repository={null} />);

    expect(await screen.findByRole("link", { name: "erinversfeldcodes/gazebolang" })).toHaveAttribute(
      "href",
      "/spend?repository=erinversfeldcodes%2Fgazebolang",
    );
  });

  it("says when nothing was spent in the period", async () => {
    render(<SpendPage client={fakeClient({ spendDays: async () => [] })} repository="o/r" />);

    expect(await screen.findByText("Nothing was spent in the last 14 days.")).toBeInTheDocument();
  });
});
