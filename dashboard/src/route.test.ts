import { describe, expect, it } from "vitest";
import { parseRoute, spendPath, ticketPath } from "./route";

describe("the dashboard's routes", () => {
  it("knows each page by its address", () => {
    expect(parseRoute("/", "")).toEqual({ page: "tickets" });
    expect(parseRoute("/tickets/erinversfeldcodes/gazebolang/12", "")).toEqual({
      page: "ticket",
      repository: "erinversfeldcodes/gazebolang",
      number: 12,
    });
    expect(parseRoute("/spend", "")).toEqual({ page: "spend", repository: null });
    expect(parseRoute("/spend", "?repository=erinversfeldcodes/gazebolang")).toEqual({
      page: "spend",
      repository: "erinversfeldcodes/gazebolang",
    });
  });

  it("does not take an address that is no page for one", () => {
    for (const path of ["/nope", "/tickets", "/tickets/a/b", "/tickets/a/b/x", "/tickets/a/b/0", "/tickets/a/b/1/2"]) {
      expect(parseRoute(path, "")).toEqual({ page: "not-found" });
    }
  });

  it("writes the addresses it reads", () => {
    expect(ticketPath("o/r", 7)).toBe("/tickets/o/r/7");
    expect(parseRoute(ticketPath("o/r", 7), "")).toEqual({ page: "ticket", repository: "o/r", number: 7 });
    expect(spendPath("o/r")).toBe("/spend?repository=o%2Fr");
    expect(parseRoute("/spend", new URL(`http://x${spendPath("o/r")}`).search)).toEqual({ page: "spend", repository: "o/r" });
    expect(spendPath(null)).toBe("/spend");
  });
});
