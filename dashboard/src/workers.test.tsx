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

    await user.type(await screen.findByLabelText("Name the worker, for example the machine it runs on"), "work");
    await user.click(screen.getByRole("button", { name: "Make a device token" }));

    expect(make).toHaveBeenCalledWith("work");
    expect(await screen.findByLabelText("Device token")).toHaveTextContent(SECRET);
    expect(screen.getByLabelText("Command")).toHaveTextContent("heph worker --control-plane https://cp.example");
    expect(container).toHaveTextContent("~/.config/hephaestium/worker-token-work");
    expect(within(screen.getByRole("table")).queryByText(SECRET)).toBeNull();

    unmount();
    const reloaded = render(<WorkersPage client={client} origin="https://cp.example" />);
    await screen.findByRole("table");
    expect(reloaded.container).not.toHaveTextContent(SECRET);
  });

  async function issue(name: string, origin = "https://cp.example") {
    const user = userEvent.setup();
    const view = render(<WorkersPage client={fakeClient()} origin={origin} />);
    await user.type(await screen.findByLabelText("Name the worker, for example the machine it runs on"), name);
    await user.click(screen.getByRole("button", { name: "Make a device token" }));
    await screen.findByLabelText("Device token");
    return { user, ...view };
  }

  it("names the file after the token, with the token already in the save command and the file in the run command", async () => {
    await issue("work");

    expect(screen.getByLabelText("Save command")).toHaveTextContent(
      `umask 077; printf '%s' '${SECRET}' > ~/.config/hephaestium/worker-token-work`,
    );
    expect(screen.getByLabelText("Command")).toHaveTextContent(
      "HEPH_WORKER_TOKEN_FILE=~/.config/hephaestium/worker-token-work heph worker --control-plane https://cp.example",
    );
  });

  it("makes a name with spaces and punctuation safe for a file name", async () => {
    await issue("Erin's Mac / mini!");

    expect(screen.getByLabelText("Command")).toHaveTextContent(
      "HEPH_WORKER_TOKEN_FILE=~/.config/hephaestium/worker-token-erin-s-mac-mini heph worker",
    );
  });

  it("never offers the default file as where to save, only as the single-worker alternative", async () => {
    await issue("mac");

    for (const label of ["Save command", "Command"]) {
      expect(screen.getByLabelText(label).textContent).not.toMatch(/worker-token(?![-\w])/);
    }
    expect(screen.getByLabelText("Command")).toHaveTextContent("worker-token-mac");
    expect(screen.getByText(/only worker/)).toHaveTextContent(
      "~/.config/hephaestium/worker-token, which heph worker reads when no file is named",
    );
  });

  describe("copying", () => {
    const blocks = [
      ["Device token", SECRET],
      ["Save command", `umask 077; printf '%s' '${SECRET}' > ~/.config/hephaestium/worker-token-work`],
      ["Command", "HEPH_WORKER_TOKEN_FILE=~/.config/hephaestium/worker-token-work heph worker --control-plane https://cp.example"],
    ] as const;

    it.each(blocks)("copies the %s and shows Copied", async (label, text) => {
      const { user } = await issue("work");
      const write = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();

      await user.click(screen.getByRole("button", { name: `Copy ${label.toLowerCase()}` }));

      expect(write).toHaveBeenCalledTimes(1);
      expect(write).toHaveBeenCalledWith(text);
      expect(screen.getByRole("button", { name: `Copy ${label.toLowerCase()}` })).toHaveTextContent("Copied");
    });

    it("selects the text and says to copy by hand where the clipboard is refused", async () => {
      const { user } = await issue("work");
      vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("denied"));

      await user.click(screen.getByRole("button", { name: "Copy device token" }));

      expect(window.getSelection()?.toString()).toBe(SECRET);
      expect(await screen.findByRole("status")).toHaveTextContent("copy it by hand");
    });
  });

  it("says the name is needed, and why the button is disabled, until one is typed", async () => {
    const user = userEvent.setup();
    render(<WorkersPage client={fakeClient()} />);
    const button = await screen.findByRole("button", { name: "Make a device token" });

    expect(button).toBeDisabled();
    expect(button).toHaveAccessibleDescription(/Type a name first/);
    await user.type(screen.getByLabelText("Name the worker, for example the machine it runs on"), "x");
    expect(button).toBeEnabled();
    expect(screen.queryByText(/Type a name first/)).toBeNull();
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

    await user.type(await screen.findByLabelText("Name the worker, for example the machine it runs on"), "x");
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
