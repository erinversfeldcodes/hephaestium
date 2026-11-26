import type { Client, TicketSummary } from "../api";
import { grouped, lastEvent, needsText } from "../board";
import { Link } from "../Link";
import { PullRequestLink } from "../PullRequestLink";
import { ticketPath } from "../route";
import { trackOf } from "../track";
import { Shown, useRemote } from "../useRemote";

/**
 * The tickets as rows grouped by what they ask of a person: those that need one, those in
 * flight and those that shipped this week. Each row says what it needs, so a person need not
 * open it, and draws where the ticket is on a six-step track. A group with nothing in it is
 * not shown.
 */
export function BoardPage({ client }: { readonly client: Client }) {
  const tickets = useRemote(() => client.tickets(), [client]);

  return (
    <section aria-labelledby="board-heading">
      <h2 id="board-heading">Board</h2>
      <Shown state={tickets}>
        {(rows) => {
          if (rows.length === 0) return <p>No ticket is waiting or has had a run yet.</p>;
          const now = new Date();
          const shown = grouped(rows, now);
          if (shown.length === 0) return <p>Nothing needs you, is in flight or shipped this week.</p>;
          return (
            <>
              <p className="small">
                What needs you, what is moving, and what shipped this week, across {new Set(rows.map((t) => t.repository)).size}{" "}
                {new Set(rows.map((t) => t.repository)).size === 1 ? "repository" : "repositories"}.
              </p>
              {shown.map((group) => (
                <section key={group.id} aria-labelledby={`group-${group.id}`} className="group">
                  <div className="group-head">
                    <h3 id={`group-${group.id}`}>{group.title}</h3>
                    <span className="count">{group.tickets.length}</span>
                  </div>
                  <ul className="rows">
                    {group.tickets.map((ticket) => (
                      <Row key={`${ticket.repository}#${ticket.number}`} ticket={ticket} now={now} />
                    ))}
                  </ul>
                </section>
              ))}
            </>
          );
        }}
      </Shown>
    </section>
  );
}

function Row({ ticket, now }: { readonly ticket: TicketSummary; readonly now: Date }) {
  const track = trackOf({ workloads: ticket.runs, pullRequest: ticket.pullRequest, settled: ticket.settledAt !== null });
  const runs = ticket.runs.filter((r) => r.kind !== "review").length;
  return (
    <li className="row" data-needs={ticket.needs?.kind}>
      <div className="title">
        <Link to={ticketPath(ticket.repository, ticket.number)}>{ticket.title ?? `#${ticket.number}`}</Link>
        <span className="where">{`${ticket.repository} #${ticket.number} · ${lastEvent(ticket, now)}`}</span>
        {ticket.needs !== null && <span className="ask">{needsText(ticket.needs, runs)}</span>}
      </div>
      <div>
        <ol className="track" aria-label="Steps">
          {track.steps.map((step) => (
            <li
              key={step.id}
              tabIndex={0}
              aria-label={step.say}
              aria-current={step.state === "now" || step.state === "bad" ? "step" : undefined}
              data-step={step.id}
              data-state={step.state}
              data-say={step.say}
            />
          ))}
        </ol>
        <div className="stage">
          <b>{track.stage}</b>
          {track.detail !== null && ` · ${track.detail}`}
        </div>
      </div>
      <div className="links">
        {ticket.pullRequest === null ? <span className="small">no PR yet</span> : <PullRequestLink pullRequest={ticket.pullRequest} />}
      </div>
      <div className="num">
        {`$${ticket.spendUsd.toFixed(2)}`}
        <small>{ticket.workloads === 0 ? "no run yet" : `${ticket.workloads} ${ticket.workloads === 1 ? "run" : "runs"}`}</small>
      </div>
    </li>
  );
}
