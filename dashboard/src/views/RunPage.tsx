import { useEffect, useState } from "react";
import type { Client, RunDetail } from "../api";
import { when } from "../format";
import { Link } from "../Link";
import { ticketPath } from "../route";
import { Shown, useRemote } from "../useRemote";
import { followEvents, type Ending, type Follow, type RunEvent } from "../stream";
import { describe, movesState, steps } from "../transcript";

type Stream =
  | { readonly kind: "following"; readonly reconnecting: number }
  | { readonly kind: "ended"; readonly how: Ending };

/**
 * A run: its state, the steps of its agents and its events as they are kept. The events are
 * read from the stream until the one that finishes the run, and the run is read again when an
 * event moves its state, so the state shown is the API's and not a copy of its rules.
 */
export function RunPage({
  client,
  id,
  follow = followEvents,
}: {
  readonly client: Client;
  readonly id: string;
  readonly follow?: Follow;
}) {
  const [events, setEvents] = useState<readonly RunEvent[]>([]);
  const [stream, setStream] = useState<Stream>({ kind: "following", reconnecting: 0 });
  const [run, setRun] = useState<RunDetail | null | undefined>(undefined);
  const [failed, setFailed] = useState(false);
  const [reads, setReads] = useState(0);
  const gates = useRemote(() => client.gates(id), [client, id, reads]);

  // The run is read when the page opens, and again whenever an event moves it on. A read that
  // fails after one has succeeded leaves what was shown.
  useEffect(() => {
    let current = true;
    client.workload(id).then(
      (value) => {
        if (current) setRun(value);
      },
      () => {
        if (current) setFailed(true);
      },
    );
    return () => {
      current = false;
    };
  }, [client, id, reads]);

  useEffect(() => {
    const controller = new AbortController();
    // A new follow begins from the first event, so what an earlier one gave is dropped.
    setEvents([]);
    setStream({ kind: "following", reconnecting: 0 });
    follow({
      workload: id,
      signal: controller.signal,
      onEvent: (event) => {
        setEvents((all) => [...all, event]);
        setStream((now) => (now.kind === "following" && now.reconnecting > 0 ? { kind: "following", reconnecting: 0 } : now));
        if (movesState(event)) setReads((n) => n + 1);
      },
      onReconnecting: (attempt) => setStream({ kind: "following", reconnecting: attempt }),
    }).then((how) => {
      if (!controller.signal.aborted) setStream({ kind: "ended", how });
    });
    return () => controller.abort();
  }, [follow, id]);

  if (run === undefined && failed) return <p role="alert">This page could not be read from the control plane.</p>;
  if (run === undefined) return <p>Loading</p>;
  if (run === null) return <p>There is no such run.</p>;

  const lines = events.flatMap((event) => {
    const line = describe(event);
    return line === null ? [] : [{ sequence: event.sequence, time: event.time, line }];
  });

  return (
    <>
      <h2>
        Run: {run.kind}{" "}
        <span className="state" data-state={run.state}>
          {run.state}
        </span>
      </h2>
      <p>
        <Link to={ticketPath(run.repository, run.ticket)}>{`${run.repository} #${run.ticket}`}</Link>
      </p>
      <p>
        Started {when(run.startedAt)}, finished {when(run.finishedAt)}
      </p>

      <p role="status">{streamText(stream)}</p>
      {stream.kind === "ended" && stream.how === "signed-out" && (
        <p>
          Your session has ended. <a href="/auth/login">Sign in with GitHub</a>
        </p>
      )}

      <section aria-labelledby="gates-heading">
        <h3 id="gates-heading">Gates</h3>
        <Shown state={gates}>
          {(all) =>
            all.length === 0 ? (
              <p>This run has kept no gates.</p>
            ) : (
              <table aria-label="Gates">
                <thead>
                  <tr>
                    <th>Gate</th>
                    <th>Result</th>
                    <th>Why</th>
                  </tr>
                </thead>
                <tbody>
                  {all.map((gate) => (
                    <tr key={gate.name}>
                      <td>{gate.name}</td>
                      <td>{gate.outcome ?? "-"}</td>
                      <td>
                        {gate.reason ?? ""}
                        {gate.ciCheck !== null &&
                          ` (left to the check ${gate.ciCheck})`}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )
          }
        </Shown>
      </section>

      <section aria-labelledby="steps-heading">
        <h3 id="steps-heading">Steps</h3>
        <ol aria-label="Steps">
          {steps(events).map((step, index) => (
            <li key={index}>
              <span>{step.name}</span> <span>{step.verdict ?? "running"}</span>
            </li>
          ))}
        </ol>
      </section>

      <section aria-labelledby="events-heading">
        <h3 id="events-heading">Events</h3>
        <ol aria-label="Events">
          {lines.map(({ sequence, time, line }) => (
            <li key={sequence}>
              <small>{when(time)}</small> {line}
            </li>
          ))}
        </ol>
      </section>
    </>
  );
}

function streamText(stream: Stream): string {
  if (stream.kind === "following") {
    return stream.reconnecting === 0 ? "Following the run." : `The connection dropped; trying again (${stream.reconnecting}).`;
  }
  switch (stream.how) {
    case "finished":
      return "The run has ended.";
    case "signed-out":
      return "The stream stopped because the session has ended.";
    case "gave-up":
      return "The stream was lost. Reload the page to follow the run again.";
    case "stopped":
      return "Stopped following.";
  }
}
