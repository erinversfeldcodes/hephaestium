import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { App } from "./App";
import type { Client } from "./api";
import { ApiError } from "./api";

function clientWith(session: Client["session"]): Client {
  return { session };
}

describe("the dashboard's first page", () => {
  it("says it is loading until the session is known", () => {
    render(<App client={clientWith(() => new Promise(() => undefined))} />);

    expect(screen.getByText("Loading")).toBeInTheDocument();
  });

  it("offers the GitHub sign-in to a person who is not signed in", async () => {
    render(<App client={clientWith(async () => null)} />);

    const link = await screen.findByRole("link", { name: "Sign in with GitHub" });
    expect(link).toHaveAttribute("href", "/auth/login");
  });

  it("shows who is signed in and a way to sign out", async () => {
    render(<App client={clientWith(async () => ({ login: "octocat" }))} />);

    expect(await screen.findByText("octocat")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Sign out" })).toHaveAttribute("href", "/auth/logout");
    expect(screen.queryByRole("link", { name: "Sign in with GitHub" })).not.toBeInTheDocument();
  });

  it("says so when the API cannot be reached, and does not pretend nobody is signed in", async () => {
    const session = vi.fn(async () => Promise.reject(new ApiError(502, "bad gateway")));
    render(<App client={clientWith(session)} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("could not be reached");
    expect(screen.queryByRole("link", { name: "Sign in with GitHub" })).not.toBeInTheDocument();
  });
});
