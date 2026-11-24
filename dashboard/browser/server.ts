// Stands in for the control plane: every request the dashboard makes is answered here from the
// fixtures, so no control plane, database or login is needed. A request nothing here answers is
// recorded, and a test fails on it, so a page cannot quietly show an error where data belongs.
import type { Page, Route } from "@playwright/test";
import type { DeviceToken } from "../src/api";
import {
  eventStream,
  wireDays,
  wireGates,
  wireQuestions,
  wireReviewEffort,
  wireRun,
  wireTicket,
  wireTickets,
  wireTokens,
  wireVerdicts,
  world,
} from "./wire";

export interface Answers {
  /** The requests nothing answered. */
  readonly unanswered: string[];
  /** Resolves once the page has gone quiet: nothing is being answered and nothing has been asked for a moment. */
  quiet(): Promise<void>;
}

const json = (body: unknown, status = 200) => ({
  status,
  contentType: "application/json",
  body: JSON.stringify(body),
});

const stubPage = { status: 200, contentType: "text/html", body: "<!doctype html><title>elsewhere</title><p>elsewhere</p>" };

export async function answerFromFixtures(page: Page, origin: string): Promise<Answers> {
  const unanswered: string[] = [];
  let tokens: DeviceToken[] = [...world.tokens];
  let lastAsked = Date.now();

  await page.route(/.*/, async (route: Route) => {
    lastAsked = Date.now();
    const request = route.request();
    const url = new URL(request.url());

    // A link out of the dashboard, and the sign-in and sign-out pages, are pages of other
    // systems: they are not loaded, and are answered with a stub so that a link can be followed.
    if (url.origin !== origin || url.pathname.startsWith("/auth/")) {
      if (request.resourceType() === "document") return route.fulfill(stubPage);
      return route.abort();
    }
    if (!url.pathname.startsWith("/api/")) {
      // Loading the page again starts from the fixtures again, as a control plane that was reset would.
      if (request.resourceType() === "document") tokens = [...world.tokens];
      return route.continue();
    }

    const parts = url.pathname.split("/").slice(2).map(decodeURIComponent);
    const post = request.method() === "POST";
    const [head, a, b, c, d] = parts;

    if (head === "session") return route.fulfill(json({ login: "octocat" }));
    if (head === "csrf") return route.fulfill(json({ csrfToken: "csrf-token" }));
    if (head === "tickets" && parts.length === 1) return route.fulfill(json(wireTickets(world.tickets)));
    if (head === "tickets" && a && b && c !== undefined) {
      if (d === undefined) return route.fulfill(json(wireTicket(world.detail)));
      if (d === "verdicts") return route.fulfill(json(wireVerdicts()));
      if (d === "review-effort") return route.fulfill(json(wireReviewEffort()));
      if (d === "reply" && post) return route.fulfill(json({ commented: true, approved: true }));
    }
    if (head === "questions") return route.fulfill(json(wireQuestions(world.questions)));
    if (head === "spend" && a === "days") return route.fulfill(json(wireDays()));
    if (head === "workloads" && a === "w-2") {
      if (b === undefined) return route.fulfill(json(wireRun()));
      if (b === "gates") return route.fulfill(json(wireGates()));
      if (b === "account") return route.fulfill(json({ account: "Guards now parse.", question: null }));
      if (b === "events") return route.fulfill({ status: 200, contentType: "text/event-stream", body: eventStream() });
    }
    if (head === "device-tokens") {
      if (a === undefined && !post) return route.fulfill(json(wireTokens(tokens)));
      if (a === undefined && post) {
        const name = (request.postDataJSON() as { name: string }).name;
        const made: DeviceToken = {
          id: `t-${tokens.length + 1}`,
          name,
          createdAt: "2026-11-24T08:00:00Z",
          lastSeenAt: null,
          revokedAt: null,
          connected: false,
          running: 0,
          slots: 1,
        };
        tokens = [made, ...tokens];
        return route.fulfill(json({ ...made, token: "heph_secret_once" }, 201));
      }
      if (a !== undefined && b === "revoke" && post) {
        tokens = tokens.map((t) => (t.id === a ? { ...t, revokedAt: "2026-11-24T08:05:00Z" } : t));
        return route.fulfill(json({}));
      }
    }

    unanswered.push(`${request.method()} ${url.pathname}${url.search}`);
    return route.fulfill(json({ error: "no fixture answers this" }, 404));
  });

  return {
    unanswered,
    async quiet() {
      // A page asks for a token, then sends what it was asked to; the wait is longer than that gap.
      while (Date.now() - lastAsked < 300) await page.waitForTimeout(50);
    },
  };
}
