import { useState } from "react";
import type { AgentText, Client, Run } from "../api";
import { amount, amounts, when } from "../format";
import { Link } from "../Link";
import { runPath } from "../route";
import { Shown, useRemote } from "../useRemote";
import { QuestionPanel } from "./QuestionPanel";

type Words =
  | { readonly kind: "closed" }
  | { readonly kind: "loading" }
  | { readonly kind: "failed" }
  | { readonly kind: "read"; readonly text: AgentText };

/** One run's words, which are read from the control plane only when a person asks. */
function RunWords({ client, run }: { readonly client: Client; readonly run: Run }) {
  const [words, setWords] = useState<Words>({ kind: "closed" });

  function read() {
    setWords({ kind: "loading" });
    client.agentText(run.id).then(
      (text) => setWords({ kind: "read", text }),
      () => setWords({ kind: "failed" }),
    );
  }

  return (
    <div role="group" id={`run-${run.id}`} aria-label={`${run.kind} run ${run.id}`}>
      <h4>
        {run.kind} <small>{when(run.createdAt)}</small>
      </h4>
      {words.kind === "closed" && (
        <button type="button" onClick={read}>
          Read what the agent said
        </button>
      )}
      {words.kind === "loading" && <p>Loading</p>}
      {words.kind === "failed" && <p role="alert">What the agent said could not be read.</p>}
      {words.kind === "read" && <Said text={words.text} />}
    </div>
  );
}

function Said({ text }: { readonly text: AgentText }) {
  if (text.account === null && text.question === null) {
    return <p>The agent said nothing in this run.</p>;
  }
  return (
    <>
      {text.question !== null && (
        <>
          <h5>The agent's question</h5>
          <pre className="agent-text">{text.question}</pre>
        </>
      )}
      {text.account !== null && (
        <>
          <h5>The agent's account</h5>
          <pre className="agent-text">{text.account}</pre>
        </>
      )}
    </>
  );
}

export function TicketDetailPage({
  client,
  repository,
  number,
}: {
  readonly client: Client;
  readonly repository: string;
  readonly number: number;
}) {
  const ticket = useRemote(() => client.ticket(repository, number), [client, repository, number]);

  return (
    <Shown state={ticket}>
      {(detail) =>
        detail === null ? (
          <p>No run has been made for this ticket.</p>
        ) : (
          <>
            <h2>
              {detail.title ?? `Ticket #${detail.number}`} <small>{`${detail.repository} #${detail.number}`}</small>
            </h2>
            {detail.url !== null && <a href={detail.url}>On GitHub</a>}

            <QuestionPanel client={client} repository={repository} number={number} />

            <section aria-labelledby="runs-heading">
              <h3 id="runs-heading">Runs</h3>
              <table>
                <thead>
                  <tr>
                    <th>Kind</th>
                    <th>State</th>
                    <th>Approved by</th>
                    <th>Made</th>
                    <th>Started</th>
                    <th>Finished</th>
                  </tr>
                </thead>
                <tbody>
                  {detail.workloads.map((run) => (
                    <tr key={run.id}>
                      <td>
                        <Link to={runPath(run.id)}>{run.kind}</Link>
                      </td>
                      <td>
                        <span className="state" data-state={run.state}>
                          {run.state}
                        </span>
                      </td>
                      <td>{run.approvedBy ?? "-"}</td>
                      <td>{when(run.createdAt)}</td>
                      <td>{when(run.startedAt)}</td>
                      <td>{when(run.finishedAt)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </section>

            <section aria-labelledby="words-heading">
              <h3 id="words-heading">What the agent said</h3>
              {detail.workloads.map((run) => (
                <RunWords key={run.id} client={client} run={run} />
              ))}
            </section>

            <section aria-labelledby="spend-heading">
              <h3 id="spend-heading">Spend</h3>
              <p>{amounts(detail.spend.totals)}</p>
              <dl>
                {Object.entries(detail.spend.byKind).map(([kind, totals]) => (
                  <div key={kind}>
                    <dt>{kind}</dt>
                    <dd>{Object.entries(totals).map(([unit, value]) => amount(unit, value)).join(" and ")}</dd>
                  </div>
                ))}
              </dl>
            </section>
          </>
        )
      }
    </Shown>
  );
}
