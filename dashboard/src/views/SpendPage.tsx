import type { Client } from "../api";
import { amounts } from "../format";
import { Link } from "../Link";
import { spendPath } from "../route";
import { Shown, useRemote } from "../useRemote";

const DAYS = 14;

export function SpendPage({
  client,
  repository,
}: {
  readonly client: Client;
  readonly repository: string | null;
}) {
  return (
    <section aria-labelledby="spend-page-heading">
      <h2 id="spend-page-heading">Spend</h2>
      {repository === null ? <ChooseRepository client={client} /> : <Days client={client} repository={repository} />}
    </section>
  );
}

// The API answers for one repository at a time, so the repositories are those the tickets are in.
function ChooseRepository({ client }: { readonly client: Client }) {
  const tickets = useRemote(() => client.tickets(), [client]);

  return (
    <Shown state={tickets}>
      {(rows) => {
        const repositories = [...new Set(rows.map((ticket) => ticket.repository))].sort();
        return repositories.length === 0 ? (
          <p>No repository has had a run yet.</p>
        ) : (
          <>
            <p>Choose a repository.</p>
            <ul>
              {repositories.map((name) => (
                <li key={name}>
                  <Link to={spendPath(name)}>{name}</Link>
                </li>
              ))}
            </ul>
          </>
        );
      }}
    </Shown>
  );
}

function Days({ client, repository }: { readonly client: Client; readonly repository: string }) {
  const days = useRemote(() => client.spendDays(repository, DAYS), [client, repository]);

  return (
    <Shown state={days}>
      {(rows) =>
        rows.length === 0 ? (
          <p>{`Nothing was spent in the last ${DAYS} days.`}</p>
        ) : (
          <>
            <p>{`${repository}, the last ${DAYS} days`}</p>
            <table>
              <thead>
                <tr>
                  <th>Day</th>
                  <th>Spent</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <tr key={row.day}>
                    <td>{row.day}</td>
                    <td>{amounts(row.totals)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        )
      }
    </Shown>
  );
}
