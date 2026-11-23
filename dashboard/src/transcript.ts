import type { RunEvent } from "./stream";

// What a person reads of a run's events. Only what the run said it did is here, as plain text:
// a tool's input and what it returned (they quote the repository's files), and the reason and
// question a finished run carries, are never put on this page. A question and the agent's own
// account are read where the dashboard shows them on purpose, on the ticket.

function str(body: Readonly<Record<string, unknown>>, key: string): string | null {
  const value = body[key];
  return typeof value === "string" ? value : null;
}

function number(body: Readonly<Record<string, unknown>>, key: string): number | null {
  const value = body[key];
  return typeof value === "number" ? value : null;
}

/** An enum as the contract writes it, `KIND_REVIEW`, in the words a person uses. */
function plain(value: string | null, prefix: string): string {
  return (value ?? "").replace(new RegExp(`^${prefix}_`), "").toLowerCase();
}

/** One line for an event, or null for one that is not worth a line (a heartbeat, a cost). */
export function describe(event: RunEvent): string | null {
  const { body } = event;
  switch (event.kind) {
    case "started":
      return `Executor ${str(body, "executorVersion") ?? ""} started`.replace("  ", " ");
    case "workspaceReady":
      return "Workspace ready";
    case "gateStarted":
      return `Gate ${str(body, "name") ?? ""} started`.replace("  ", " ");
    case "gateFinished": {
      const result = body["result"];
      const outcome =
        typeof result === "object" && result !== null ? str(result as Record<string, unknown>, "outcome") : null;
      return `Gate ${str(body, "name") ?? ""}: ${outcome ?? "finished"}`.replace("  ", " ");
    }
    case "stepStarted":
      return `Step started: ${stepName(body)}`;
    case "stepFinished":
      return `Step finished: ${stepName(body)}`;
    case "agentMessage":
      return str(body, "text");
    case "toolCall":
      return `Called ${str(body, "name") ?? "a tool"}`;
    case "toolResult":
      return body["isError"] === true ? "A tool returned an error" : "A tool returned";
    case "skillLoaded":
      return `Loaded skill ${str(body, "name") ?? ""}`.trim();
    case "pullRequestOpened":
      return `Opened pull request #${str(body, "number") ?? ""}`.trim();
    case "deliveryBegan":
      return "Began delivering";
    case "finished":
      return `Finished: ${plain(str(body, "outcome"), "OUTCOME")}`;
    case "heartbeat":
    case "cost":
      return null;
    default:
      return event.kind;
  }
}

/** A step's kind, round and lens in words: `review, round 1, rust`. */
export function stepName(body: Readonly<Record<string, unknown>>): string {
  const round = number(body, "round");
  const lens = str(body, "lens");
  return [plain(str(body, "kind"), "KIND"), round === null ? null : `round ${round}`, lens]
    .filter((part): part is string => part !== null && part !== "")
    .join(", ");
}

export interface Step {
  readonly name: string;
  /** What the step ended with, or null while it runs. */
  readonly verdict: string | null;
}

/** The steps a run's events tell of, in the order they began. */
export function steps(events: readonly RunEvent[]): readonly Step[] {
  const found: { name: string; verdict: string | null }[] = [];
  for (const event of events) {
    if (event.kind !== "stepStarted" && event.kind !== "stepFinished") continue;
    const name = stepName(event.body);
    const open = found.find((step) => step.name === name && step.verdict === null);
    if (event.kind === "stepStarted") {
      found.push({ name, verdict: null });
    } else if (open) {
      open.verdict = str(event.body, "verdict") ?? "finished";
    }
  }
  return found;
}

/** Events that move the run on, after which its state is read again. */
export function movesState(event: RunEvent): boolean {
  return event.kind === "started" || event.kind === "workspaceReady" || event.kind === "gateStarted" || event.kind === "finished";
}
