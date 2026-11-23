import type { Client } from "../api";
import { when } from "../format";
import { Link } from "../Link";
import { ticketPath } from "../route";
import { Shown, useRemote } from "../useRemote";

export function TicketListPage({ client }: { readonly client: Client }) {
  const tickets = useRemote(() => client.tickets(), [client]);

  return (
    <section aria-labelledby="tickets-heading">
      <h2 id="tickets-heading">Tickets</h2>
      <Shown state={tickets}>
        {(all) => {
          // A ticket waiting for its first run is on the board; this list is of runs.
          const rows = all.flatMap((ticket) => (ticket.latest ? [{ ...ticket, latest: ticket.latest }] : []));
          return rows.length === 0 ? (
            <p>No ticket has had a run yet.</p>
          ) : (
            <div className="table-scroll">
<table>
              <thead>
                <tr>
                  <th>Ticket</th>
                  <th>Latest run</th>
                  <th>State</th>
                  <th>Runs</th>
                  <th>When</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((ticket) => (
                  <tr key={`${ticket.repository}#${ticket.number}`}>
                    <td>
                      <Link to={ticketPath(ticket.repository, ticket.number)}>
                        {ticket.title ?? `#${ticket.number}`}
                      </Link>
                      <br />
                      <small>{`${ticket.repository} #${ticket.number}`}</small>
                    </td>
                    <td>{ticket.latest.kind}</td>
                    <td>
                      <span className="state" data-state={ticket.latest.state}>
                        {ticket.latest.state}
                      </span>
                    </td>
                    <td>{ticket.workloads}</td>
                    <td>{when(ticket.latest.at)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
</div>
          );
        }}
      </Shown>
    </section>
  );
}
