import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { createClient } from "./api";
import { deviceTokens, fakeClient } from "./fixtures";
import { parseRoute } from "./route";
import { WorkersPage } from "./views/WorkersPage";

const SECRET = "heph_secret_once";

describe("the workers page", () => {
  it("lists each token with its name, when it was made, when it last connected and whether it is connected now", async () => {
    render(<WorkersPage client={fakeClient()} />);

    const laptop = await screen.findByRole("row", { name: /laptop/ });
    expect(within(laptop).getAllByRole("cell")[3]).toHaveTextContent("yes");
    const desk = screen.getByRole("row", { name: /desk/ });
    expect(within(desk).getAllByRole("cell")[2]).toHaveTextContent("-");
    expect(within(desk).getAllByRole("cell")[3]).toHaveTextContent("no");
  });

  it("shows how many workloads each worker is running and how many it may run", async () => {
    render(<WorkersPage client={fakeClient()} />);

    const laptop = await screen.findByRole("row", { name: /laptop/ });
    expect(within(laptop).getAllByRole("cell")[4]).toHaveTextContent("2 of 3");
    const desk = screen.getByRole("row", { name: /desk/ });
    expect(within(desk).getAllByRole("cell")[4]).toHaveTextContent("0 of 1");
  });

  it("shows a revoked token as revoked, with nothing to revoke", async () => {
    const revoked = [{ ...deviceTokens[0]!, revokedAt: "2026-11-22T00:00:00Z", connected: false }];
    render(<WorkersPage client={fakeClient({ deviceTokens: async () => revoked })} />);

    const row = await screen.findByRole("row", { name: /laptop/ });
    expect(within(row).getAllByRole("cell")[3]).toHaveTextContent("revoked");
    expect(within(row).queryByRole("button")).toBeNull();
  });

  it("shows a new token once, with the command and the file, and not in the list or after a reload", async () => {
    const user = userEvent.setup();
    const make = vi.fn(fakeClient().makeDeviceToken);
    const client = fakeClient({ makeDeviceToken: make });
    const { unmount, container } = render(<WorkersPage client={client} origin="https://cp.example" />);

    await user.type(await screen.findByLabelText("Name"), "work");
    await user.click(screen.getByRole("button", { name: "Make a device token" }));

    expect(make).toHaveBeenCalledWith("work");
    expect(await screen.findByLabelText("Device token")).toHaveTextContent(SECRET);
    expect(screen.getByLabelText("Command")).toHaveTextContent("heph worker --control-plane https://cp.example");
    expect(container).toHaveTextContent("~/.config/hephaestium/worker-token");
    expect(within(screen.getByRole("table")).queryByText(SECRET)).toBeNull();

    unmount();
    const reloaded = render(<WorkersPage client={client} origin="https://cp.example" />);
    await screen.findByRole("table");
    expect(reloaded.container).not.toHaveTextContent(SECRET);
  });

  it("never puts a token in the list, even when the API's list carried one (planted violation)", async () => {
    const leaky = { tokens: [{ ...deviceTokens[0], token: SECRET }], connected: true };
    const fetcher = vi.fn(async () => new Response(JSON.stringify(leaky), { status: 200 }));
    const { container } = render(<WorkersPage client={createClient(fetcher)} />);

    await screen.findByRole("row", { name: /laptop/ });
    expect(container).not.toHaveTextContent(SECRET);
  });

  it("revokes a token and lists again", async () => {
    const user = userEvent.setup();
    let rows = [...deviceTokens];
    const revoke = vi.fn(async (id: string) => {
      rows = rows.filter((row) => row.id !== id);
      return { ok: true as const, value: null };
    });
    render(<WorkersPage client={fakeClient({ deviceTokens: async () => rows, revokeDeviceToken: revoke })} />);

    await user.click(await screen.findByRole("button", { name: "Revoke desk" }));

    expect(revoke).toHaveBeenCalledWith("t-2");
    await vi.waitFor(() => expect(screen.queryByRole("row", { name: /desk/ })).toBeNull());
  });

  it("says why a token was not made", async () => {
    const user = userEvent.setup();
    const refused = async () => ({ ok: false as const, error: "a name of one to 80 characters is needed" });
    render(<WorkersPage client={fakeClient({ makeDeviceToken: refused })} />);

    await user.type(await screen.findByLabelText("Name"), "x");
    await user.click(screen.getByRole("button", { name: "Make a device token" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("80 characters");
  });

  it("is the page at /workers", () => {
    expect(parseRoute("/workers", "")).toEqual({ page: "workers" });
  });
});

describe("the client's writes to device tokens", () => {
  function server(handler: (url: string, init?: RequestInit) => Response) {
    return vi.fn(async (url: RequestInfo | URL, init?: RequestInit) => handler(String(url), init));
  }

  it("makes a token with the CSRF token, reading the secret once from the answer", async () => {
    const fetcher = server((url) =>
      url === "/api/csrf"
        ? new Response(JSON.stringify({ csrfToken: "c1" }))
        : new Response(JSON.stringify({ ...deviceTokens[1], token: SECRET }), { status: 201 }),
    );

    const result = await createClient(fetcher).makeDeviceToken("desk");

    expect(result).toMatchObject({ ok: true, value: { secret: SECRET, token: { id: "t-2" } } });
    const [url, init] = fetcher.mock.calls[1] ?? [];
    expect(url).toBe("/api/device-tokens");
    expect(init?.method).toBe("POST");
    expect((init?.headers as Record<string, string>)["x-csrf-token"]).toBe("c1");
    expect(init?.body).toBe(JSON.stringify({ name: "desk" }));
  });

  it("revokes with the CSRF token, and writes nothing without one", async () => {
    const fetcher = server((url) =>
      url === "/api/csrf" ? new Response("{}", { status: 401 }) : new Response("{}"),
    );
    expect(await createClient(fetcher).revokeDeviceToken("t-1")).toMatchObject({ ok: false });
    expect(fetcher).toHaveBeenCalledTimes(1);

    const good = server((url) =>
      url === "/api/csrf" ? new Response(JSON.stringify({ csrfToken: "c1" })) : new Response(JSON.stringify({ revoked: true })),
    );
    expect(await createClient(good).revokeDeviceToken("t-1")).toEqual({ ok: true, value: null });
    expect(good.mock.calls[1]?.[0]).toBe("/api/device-tokens/t-1/revoke");
  });
});
