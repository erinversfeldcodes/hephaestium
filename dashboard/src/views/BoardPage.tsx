import type { Client, Question } from "../api";
import { columns, lanes, waitedFor } from "../board";
import { Link } from "../Link";
import { ticketPath } from "../route";
import { Shown, useRemote } from "../useRemote";

/**
 * The tickets in a lane for each repository and a column for each state, with a marker on a
 * ticket where a person is needed. The question's text is not here: it is read on the ticket.
 * Without the questions the board still shows, and says the markers are missing.
 */
export function BoardPage({ client }: { readonly client: Client }) {
  const tickets = useRemote(() => client.tickets(), [client]);
  const questions = useRemote(() => client.questions(), [client]);

  const waiting = new Set<string>(
    questions.kind === "loaded" ? questions.value.map((q: Question) => `${q.repository}#${q.ticket}`) : [],
  );

  return (
    <section aria-labelledby="board-heading">
      <h2 id="board-heading">Board</h2>
      {questions.kind === "failed" && (
        <p role="status">Where a person is needed could not be read, so no ticket is marked.</p>
      )}
      <Shown state={tickets}>
        {(rows) =>
          rows.length === 0 ? (
            <p>No ticket is waiting or has had a run yet.</p>
          ) : (
            lanes(rows).map((lane) => (
              <section key={lane.repository} aria-label={lane.repository} className="lane">
                <h3>{lane.repository}</h3>
                <div className="columns">
                  {columns.map((column) => {
                    const cards = lane.cards.get(column.id) ?? [];
                    if (column.id === "unknown" && cards.length === 0) return null;
                    return (
                      <div key={column.id} role="group" aria-label={column.title} className="column">
                        <h4>{column.title}</h4>
                        <ul>
                          {cards.map((ticket) => (
                            <li key={ticket.number}>
                              <Link to={ticketPath(ticket.repository, ticket.number)}>
                                {ticket.title ?? `#${ticket.number}`}
                              </Link>{" "}
                              <small>{`#${ticket.number}`}</small>
                              {waiting.has(`${ticket.repository}#${ticket.number}`) && (
                                <strong className="needed"> Person needed</strong>
                              )}
                              {ticket.waiting && (
                                <small className="waited">
                                  {" "}
                                  {waitedFor(ticket.waiting.since, new Date())}
                                  {ticket.waiting.noWorker && "; no worker has claimed it"}
                                </small>
                              )}
                            </li>
                          ))}
                        </ul>
                      </div>
                    );
                  })}
                </div>
              </section>
            ))
          )
        }
      </Shown>
    </section>
  );
}
