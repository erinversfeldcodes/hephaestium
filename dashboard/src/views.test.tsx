import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ApiError, type ReplyResult } from "./api";
import { days, detail, fakeClient, question, replied, summary } from "./fixtures";
import { SpendPage } from "./views/SpendPage";
import { BoardPage } from "./views/BoardPage";
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

  it("reads nothing of what the agent said until a person asks for a run's words", async () => {
    const agentText = vi.fn(async () => ({ account: "It is in a.ex.", question: null }));
    render(
      <TicketDetailPage
        client={fakeClient({ agentText })}
        repository="erinversfeldcodes/gazebolang"
        number={12}
      />,
    );

    await screen.findByRole("heading", { level: 2 });
    expect(agentText).not.toHaveBeenCalled();
    expect(screen.queryByText("It is in a.ex.")).not.toBeInTheDocument();

    const run = screen.getByRole("group", { name: /implementation/ });
    expect(run).toHaveAttribute("id", "run-w-1");
    await userEvent.click(within(run).getByRole("button", { name: "Read what the agent said" }));

    expect(await within(run).findByText("It is in a.ex.")).toBeInTheDocument();
    expect(agentText).toHaveBeenCalledExactlyOnceWith("w-1");
  });

  it("shows a run's question as well as its account, as text and not as markup", async () => {
    const client = fakeClient({
      agentText: async () => ({ account: "<b>bold</b> account", question: "Which database?" }),
    });
    render(<TicketDetailPage client={client} repository="erinversfeldcodes/gazebolang" number={12} />);

    const run = await screen.findByRole("group", { name: /implementation/ });
    await userEvent.click(within(run).getByRole("button", { name: "Read what the agent said" }));

    expect(await within(run).findByText("<b>bold</b> account")).toBeInTheDocument();
    expect(within(run).getByText("Which database?")).toBeInTheDocument();
    expect(within(run).getByText("The agent's account")).toBeInTheDocument();
    expect(within(run).getByText("The agent's question")).toBeInTheDocument();
  });

  it("says when a run said nothing and asked nothing", async () => {
    render(
      <TicketDetailPage
        client={fakeClient({ agentText: async () => ({ account: null, question: null }) })}
        repository="erinversfeldcodes/gazebolang"
        number={12}
      />,
    );

    const run = await screen.findByRole("group", { name: /implementation/ });
    await userEvent.click(within(run).getByRole("button", { name: "Read what the agent said" }));

    expect(await within(run).findByText("The agent said nothing in this run.")).toBeInTheDocument();
  });

  it("says when the words could not be read, without hiding the rest of the page", async () => {
    render(
      <TicketDetailPage
        client={fakeClient({
          agentText: async () => {
            throw new ApiError(500, "boom");
          },
        })}
        repository="erinversfeldcodes/gazebolang"
        number={12}
      />,
    );

    const run = await screen.findByRole("group", { name: /implementation/ });
    await userEvent.click(within(run).getByRole("button", { name: "Read what the agent said" }));

    expect(await within(run).findByRole("alert")).toHaveTextContent("could not be read");
    expect(screen.getByRole("region", { name: "Spend" })).toBeInTheDocument();
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

describe("a question waiting for a person", () => {
  it("shows the question on the ticket page as text, and not as markup", async () => {
    const asked = { ...question, question: "Use <b>Postgres</b>?\nOr SQLite?" };
    render(<TicketDetailPage client={fakeClient({ question: async () => asked })} repository="o/r" number={12} />);

    const panel = await screen.findByRole("region", { name: "Question" });
    expect(panel).toHaveTextContent("Use <b>Postgres</b>?");
    expect(panel.querySelector("b")).toBeNull();
  });

  it("shows no question, and no reply box, when none waits", async () => {
    render(<TicketDetailPage client={fakeClient()} repository="o/r" number={12} />);

    await screen.findByRole("region", { name: "Runs" });
    expect(screen.queryByRole("region", { name: "Question" })).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("does not send an empty reply", async () => {
    render(<TicketDetailPage client={fakeClient({ question: async () => question })} repository="o/r" number={12} />);

    expect(await screen.findByRole("button", { name: "Send answer" })).toBeDisabled();
  });

  it("sends the reply and says it was commented and approved as them", async () => {
    const reply = vi.fn(async () => replied);
    const user = userEvent.setup();
    render(
      <TicketDetailPage client={fakeClient({ question: async () => question, reply })} repository="o/r" number={12} />,
    );

    await user.type(await screen.findByRole("textbox", { name: "Your answer" }), "Use Postgres.");
    await user.click(screen.getByRole("button", { name: "Send answer" }));

    expect(reply).toHaveBeenCalledExactlyOnceWith("erinversfeldcodes/gazebolang", 12, "Use Postgres.");
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Your answer was posted on the ticket, and you approved it again as yourself.",
    );
    expect(screen.queryByRole("textbox")).toBeNull();
  });

  it("sends once however many times the button is pressed while it is sending", async () => {
    let finish: (result: ReplyResult) => void = () => undefined;
    const reply = vi.fn(() => new Promise<ReplyResult>((resolve) => (finish = resolve)));
    const user = userEvent.setup();
    render(
      <TicketDetailPage client={fakeClient({ question: async () => question, reply })} repository="o/r" number={12} />,
    );

    await user.type(await screen.findByRole("textbox", { name: "Your answer" }), "Yes");
    await user.click(screen.getByRole("button", { name: "Send answer" }));
    await user.click(screen.getByRole("button", { name: "Sending" }));

    expect(reply).toHaveBeenCalledTimes(1);
    finish(replied);
    await screen.findByRole("status");
  });

  it("says nothing was sent when the person may not write, and keeps what they typed", async () => {
    const refused: ReplyResult = {
      commented: false,
      approved: false,
      error: "you need write access to this repository to answer; nothing was sent",
    };
    const user = userEvent.setup();
    render(
      <TicketDetailPage
        client={fakeClient({ question: async () => question, reply: async () => refused })}
        repository="o/r"
        number={12}
      />,
    );

    await user.type(await screen.findByRole("textbox", { name: "Your answer" }), "Use Postgres.");
    await user.click(screen.getByRole("button", { name: "Send answer" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("you need write access");
    expect(screen.getByRole("textbox", { name: "Your answer" })).toHaveValue("Use Postgres.");
  });

  it("says that the comment was made and the approval was not, when only that half went", async () => {
    const half: ReplyResult = { commented: true, approved: false, error: "GitHub refused the approval" };
    const user = userEvent.setup();
    render(
      <TicketDetailPage
        client={fakeClient({ question: async () => question, reply: async () => half })}
        repository="o/r"
        number={12}
      />,
    );

    await user.type(await screen.findByRole("textbox", { name: "Your answer" }), "x");
    await user.click(screen.getByRole("button", { name: "Send answer" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Your comment was posted but the ticket is not approved");
  });

  it("is quiet when the question cannot be read: the rest of the page still shows", async () => {
    render(
      <TicketDetailPage
        client={fakeClient({ question: async () => Promise.reject(new ApiError(500, "x")) })}
        repository="o/r"
        number={12}
      />,
    );

    expect(await screen.findByRole("region", { name: "Runs" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).toBeNull();
  });
});

describe("the board", () => {
  const rows = [
    summary,
    { ...summary, number: 13, title: "Waiting one", latest: { ...summary.latest, state: "queued" } },
    { ...summary, number: 14, title: "Asked one", latest: { ...summary.latest, state: "blocked" } },
    { ...summary, number: 5, title: "Other repo", repository: "o/other", latest: { ...summary.latest, state: "completed" } },
  ];

  it("has a lane for each repository and a column for each state, with each ticket in its column", async () => {
    render(<BoardPage client={fakeClient({ tickets: async () => rows })} />);

    const lane = await screen.findByRole("region", { name: "erinversfeldcodes/gazebolang" });
    expect(within(within(lane).getByRole("group", { name: "Working" })).getByRole("link", { name: "Teach the parser about guards" })).toBeInTheDocument();
    expect(within(within(lane).getByRole("group", { name: "Queued" })).getByRole("link", { name: "Waiting one" })).toBeInTheDocument();
    expect(within(within(lane).getByRole("group", { name: "Blocked" })).getByRole("link", { name: "Asked one" })).toBeInTheDocument();
    expect(within(lane).queryByRole("link", { name: "Other repo" })).not.toBeInTheDocument();
    const other = screen.getByRole("region", { name: "o/other" });
    expect(within(within(other).getByRole("group", { name: "Completed" })).getByRole("link", { name: "Other repo" })).toHaveAttribute(
      "href",
      "/tickets/o/other/5",
    );
  });

  it("marks, and only, the tickets where a person is needed, without the question's text", async () => {
    const asked = { ...question, ticket: 14 };
    render(<BoardPage client={fakeClient({ tickets: async () => rows, questions: async () => [asked] })} />);

    const card = (await screen.findByRole("link", { name: "Asked one" })).closest("li");
    expect(card).not.toBeNull();
    expect(within(card as HTMLElement).getByText("Person needed")).toBeInTheDocument();
    expect(screen.getAllByText("Person needed")).toHaveLength(1);
    expect(screen.queryByText(question.question)).not.toBeInTheDocument();
  });

  it("still shows the board when the questions cannot be read, and says so", async () => {
    render(
      <BoardPage
        client={fakeClient({ tickets: async () => rows, questions: async () => Promise.reject(new ApiError(500, "x")) })}
      />,
    );

    expect(await screen.findByRole("link", { name: "Asked one" })).toBeInTheDocument();
    expect(await screen.findByRole("status")).toHaveTextContent("could not be read");
  });

  it("says when there is nothing, loading, and failure like the other pages", async () => {
    const { unmount } = render(<BoardPage client={fakeClient({ tickets: async () => [] })} />);
    expect(await screen.findByText("No ticket has had a run yet.")).toBeInTheDocument();
    unmount();

    render(<BoardPage client={fakeClient({ tickets: async () => Promise.reject(new ApiError(500, "x")) })} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("could not be read");
  });
});

describe("a ticket's runs", () => {
  it("each open the run's own page", async () => {
    render(<TicketDetailPage client={fakeClient()} repository="erinversfeldcodes/gazebolang" number={12} />);

    expect(await screen.findByRole("link", { name: "implementation" })).toHaveAttribute("href", "/runs/w-1");
  });
});
