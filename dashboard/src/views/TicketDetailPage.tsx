import type { Client } from "../api";
import { amount, amounts, when } from "../format";
import { Shown, useRemote } from "../useRemote";

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
                      <td>{run.kind}</td>
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
