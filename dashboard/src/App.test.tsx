import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { ApiError, createClient } from "./api";
import { fakeClient } from "./fixtures";

beforeEach(() => {
  window.history.pushState({}, "", "/");
});

describe("the dashboard", () => {
  it("says it is loading until the session is known", () => {
    render(<App client={fakeClient({ session: () => new Promise(() => undefined) })} />);

    expect(screen.getByText("Loading")).toBeInTheDocument();
  });

  it("offers the GitHub sign-in to a person who is not signed in, and shows no data", async () => {
    render(<App client={fakeClient({ session: async () => null })} />);

    const link = await screen.findByRole("link", { name: "Sign in with GitHub" });
    expect(link).toHaveAttribute("href", "/auth/login");
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });

  it("shows who is signed in, a way to sign out, and the tickets", async () => {
    render(<App client={fakeClient()} />);

    expect(await screen.findByText("octocat")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Sign out" })).toHaveAttribute("href", "/auth/logout");
    expect(await screen.findByRole("link", { name: "Teach the parser about guards" })).toBeInTheDocument();
  });

  it("says so when the API cannot be reached, and does not pretend nobody is signed in", async () => {
    const session = vi.fn(async () => Promise.reject(new ApiError(502, "bad gateway")));
    render(<App client={fakeClient({ session })} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("could not be reached");
    expect(screen.queryByRole("link", { name: "Sign in with GitHub" })).not.toBeInTheDocument();
  });

  it("opens a ticket from the list without leaving the page, and the back button returns", async () => {
    const user = userEvent.setup();
    render(<App client={fakeClient()} />);

    await user.click(await screen.findByRole("link", { name: "Teach the parser about guards" }));

    expect(await screen.findByRole("region", { name: "Spend" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/tickets/erinversfeldcodes/gazebolang/12");

    window.history.back();
    expect(await screen.findByRole("link", { name: "Teach the parser about guards" })).toBeInTheDocument();
    expect(window.location.pathname).toBe("/");
  });

  it("opens the page its address names, for a reload or a bookmark", async () => {
    window.history.pushState({}, "", "/spend?repository=erinversfeldcodes%2Fgazebolang");
    render(<App client={fakeClient()} />);

    expect(await screen.findByRole("row", { name: /2026-11-19/ })).toBeInTheDocument();
  });

  it("says when an address is no page", async () => {
    window.history.pushState({}, "", "/nowhere");
    render(<App client={fakeClient()} />);

    expect(await screen.findByText("There is no such page.")).toBeInTheDocument();
  });

  it("makes no request that writes, whichever page is open", async () => {
    const answers: Record<string, unknown> = {
      "/api/session": { login: "octocat" },
      "/api/tickets?limit=200": {
        tickets: [
          {
            repository: "o/r",
            number: 1,
            title: "T",
            workloads: 1,
            settled_at: null,
            latest: { id: "w", state: "queued", kind: "implementation", at: "2026-11-20T10:00:00Z" },
          },
        ],
      },
      "/api/tickets/o/r/1": {
        repository: "o/r",
        number: 1,
        title: "T",
        url: "https://github.com/o/r/issues/1",
        workloads: [
          {
            id: "w",
            repository: "o/r",
            ticket_number: 1,
            kind: "implementation",
            state: "queued",
            reason: null,
            parent_id: null,
            approved_by: null,
            started_at: null,
            finished_at: null,
            settled_at: null,
            created_at: "2026-11-20T10:00:00Z",
          },
        ],
        spend: { totals: { usd: 1 }, by_kind: { implementation: { usd: 1 } }, workloads: 1, parts: null },
      },
      "/api/spend/days?repository=o%2Fr&days=14": { days: [{ day: "2026-11-20", totals: { usd: 1 } }] },
    };
    const fetcher = vi.fn<typeof fetch>(async (input) => {
      const body = answers[String(input)];
      return body === undefined
        ? new Response("{}", { status: 404 })
        : new Response(JSON.stringify(body), { status: 200 });
    });
    const user = userEvent.setup();
    render(<App client={createClient(fetcher)} />);

    await user.click(await screen.findByRole("link", { name: "T" }));
    await screen.findByRole("region", { name: "Spend" });
    await user.click(screen.getByRole("link", { name: "Spend" }));
    await user.click(await screen.findByRole("link", { name: "o/r" }));
    await screen.findByRole("row", { name: /2026-11-20/ });

    await waitFor(() => expect(fetcher.mock.calls.length).toBeGreaterThanOrEqual(5));
    for (const [, init] of fetcher.mock.calls) {
      expect(init?.method).toBe("GET");
      expect(init?.body).toBeUndefined();
    }
    expect(fetcher.mock.calls.map(([url]) => String(url))).toContain("/api/tickets/o/r/1");
  });

  it("opens the board from the navigation", async () => {
    const user = userEvent.setup();
    render(<App client={fakeClient()} />);

    await user.click(await screen.findByRole("link", { name: "Board" }));

    expect(window.location.pathname).toBe("/board");
    expect(await screen.findByRole("group", { name: "Working" })).toBeInTheDocument();
  });

  it("marks the current page in the navigation", async () => {
    const user = userEvent.setup();
    render(<App client={fakeClient()} />);

    await user.click(await screen.findByRole("link", { name: "Board" }));

    expect(screen.getByRole("link", { name: "Board" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("link", { name: "Tickets" })).not.toHaveAttribute("aria-current");
  });
});
