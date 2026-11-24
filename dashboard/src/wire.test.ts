import { createClient } from "./api";
import { runDetail, days, deviceTokens, gates, reviewEffort, verdicts } from "./fixtures";
import {
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
} from "../browser/wire";

// The browser tests answer the API from the fixtures through these. A check that the real client
// reads them back as the fixtures is what shows the browser tests are shown the fixtures.
function answering(path: string, body: unknown): typeof fetch {
  return async (input) => {
    if (String(input) !== path) return new Response(null, { status: 404 });
    return new Response(JSON.stringify(body), { status: 200 });
  };
}

describe("the browser tests' answers", () => {
  it("are read back by the client as the fixtures they were written from", async () => {
    const repository = "erinversfeldcodes/gazebolang";
    const read = (path: string, body: unknown) => createClient(answering(path, body));

    expect(await read("/api/tickets?limit=200", wireTickets(world.tickets)).tickets()).toEqual(world.tickets);
    expect(await read("/api/tickets/" + repository + "/12", wireTicket(world.detail)).ticket(repository, 12)).toEqual(world.detail);
    expect(await read("/api/workloads/w-2", wireRun()).workload("w-2")).toEqual(runDetail);
    expect(await read("/api/questions", wireQuestions(world.questions)).questions()).toEqual(world.questions);
    expect(await read("/api/workloads/w-2/gates", wireGates()).gates("w-2")).toEqual(gates);
    expect(await read(`/api/tickets/${repository}/12/verdicts`, wireVerdicts()).verdicts(repository, 12)).toEqual(verdicts);
    expect(await read(`/api/tickets/${repository}/12/review-effort`, wireReviewEffort()).reviewEffort(repository, 12)).toEqual(reviewEffort);
    expect(await read(`/api/spend/days?repository=${encodeURIComponent(repository)}&days=14`, wireDays()).spendDays(repository, 14)).toEqual(days);
    expect(await read("/api/device-tokens", wireTokens(deviceTokens)).deviceTokens()).toEqual(deviceTokens);
  });
});
