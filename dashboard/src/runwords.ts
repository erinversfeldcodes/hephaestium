/** What a run's state is called on the page, and what it means. "Done" is never one of them: it is the ticket's word. */
export interface RunWord {
  readonly word: string;
  readonly meaning: string;
  /** How the bar for the run is coloured. */
  readonly tone: "running" | "ok" | "bad" | "idle";
}

const words: Readonly<Record<string, RunWord>> = {
  queued: { word: "waiting", meaning: "Waiting for a worker to take it.", tone: "idle" },
  preparing: { word: "running", meaning: "An agent is working on it now.", tone: "running" },
  running: { word: "running", meaning: "An agent is working on it now.", tone: "running" },
  gating: { word: "running", meaning: "The agent has finished and the gates are being run.", tone: "running" },
  blocked: { word: "waiting", meaning: "Waiting for a person to answer the agent's question.", tone: "idle" },
  completed: { word: "finished", meaning: "The run completed. It says nothing of whether the ticket is done.", tone: "ok" },
  // Older records and other tools write a completed run as succeeded or done; on this page it is finished.
  succeeded: { word: "finished", meaning: "The run completed. It says nothing of whether the ticket is done.", tone: "ok" },
  done: { word: "finished", meaning: "The run completed. It says nothing of whether the ticket is done.", tone: "ok" },
  failed: { word: "failed", meaning: "The run ended in failure.", tone: "bad" },
  cancelled: { word: "stopped", meaning: "The run was cancelled, or handed back when its worker stopped.", tone: "bad" },
  timed_out: { word: "timed out", meaning: "The run took longer than it was allowed.", tone: "bad" },
};

/** A run's state in run words; a state not known here is shown as it is, with no claim about it. */
export function runWord(state: string): RunWord {
  return words[state] ?? { word: state.replace(/_/g, " "), meaning: "The control plane's own state for this run.", tone: "idle" };
}

/** Whether a run is going on now. */
export function isActive(state: string): boolean {
  return runWord(state).tone === "running";
}
