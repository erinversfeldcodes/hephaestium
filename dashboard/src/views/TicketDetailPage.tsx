import { useState } from "react";
import type { AgentText, Client, Gate, ReviewEffort, Run, TicketDetail, Verdict } from "../api";
import { ago, amount, amounts, when } from "../format";
import { isActive, runWord } from "../runwords";
import { span, timelineOf } from "../timeline";
import { trackOf, stageWords } from "../track";
import { TableScroll } from "../TableScroll";
import { Link } from "../Link";
import { runPath } from "../route";
import { Shown, useRemote, type Remote } from "../useRemote";
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

function VerdictHistory({
  verdicts,
}: {
  readonly verdicts: readonly Verdict[];
}) {
  if (verdicts.length === 0)
    return <p>No verdict has been given for this ticket.</p>;
  return (
    <ol aria-label="Verdicts">
      {verdicts.map((verdict) => (
        <li key={verdict.workload}>
          <strong>{verdict.verdict}</strong> <small>{when(verdict.at)}</small>
          <ul>
            {verdict.requirements.map((requirement, index) => (
              <li key={index}>
                {requirement.status}
                {requirement.text !== null && `: ${requirement.text}`}
                {requirement.evidenceKind !== null &&
                  ` (${requirement.evidenceKind}`}
                {requirement.evidenceKind !== null &&
                  requirement.evidence !== null &&
                  `: ${requirement.evidence}`}
                {requirement.evidenceKind !== null && ")"}
              </li>
            ))}
          </ul>
        </li>
      ))}
    </ol>
  );
}

function ReviewEffortPanel({ effort }: { readonly effort: ReviewEffort }) {
  if (
    effort.reviews === 0 &&
    effort.revisions === 0 &&
    effort.localRounds === 0 &&
    effort.lenses.length === 0
  ) {
    return <p>This ticket has not been reviewed.</p>;
  }
  return (
    <>
      <p>
        {effort.localRounds} rounds before the push, {effort.reviews} reviews
        and {effort.revisions} revisions after it.
      </p>
      <ul className="lens-bars" aria-label="Findings by lens, resolved of found">
        {effort.lenses.map((lens) => (
          <li key={lens.lens}>
            <span>{lens.lens}</span>
            <i className="bar-small" role="img" aria-label={`${lens.resolved} of ${lens.found} resolved`}>
              <b style={{ width: `${lens.found === 0 ? 0 : (lens.resolved / lens.found) * 100}%` }} />
            </i>
            <span className="small">{lens.resolved}/{lens.found}</span>
          </li>
        ))}
      </ul>
      <TableScroll>
<table aria-label="Findings by lens">
        <thead>
          <tr>
            <th>Lens</th>
            <th>Reviews</th>
            <th>Found</th>
            <th>Resolved</th>
            <th>Still there</th>
          </tr>
        </thead>
        <tbody>
          {effort.lenses.map((lens) => (
            <tr key={lens.lens}>
              <td>{lens.lens}</td>
              <td>{lens.reviews}</td>
              <td>{lens.found}</td>
              <td>{lens.resolved}</td>
              <td>{lens.remaining}</td>
            </tr>
          ))}
        </tbody>
      </table>
</TableScroll>
      {effort.lenses.map((lens) =>
        lens.findings.length === 0 ? null : (
          <ul key={lens.lens} aria-label={`${lens.lens} findings`}>
            {lens.findings.map((finding, index) => (
              <li key={index}>
                {finding.title} ({finding.blocking ? "blocking" : "advisory"}
                {finding.severity !== null && `, ${finding.severity}`}):{" "}
                {finding.state}
              </li>
            ))}
          </ul>
        ),
      )}
    </>
  );
}

/** The rounds of review a ticket has had: those before the push, and each after it that a revision answered. */
function reviewRounds(effort: ReviewEffort): number {
  return effort.localRounds + (effort.reviews > 0 ? effort.revisions + 1 : 0);
}

function Tile({ name, value, note }: { readonly name: string; readonly value: string; readonly note: string }) {
  return (
    <div className="tile">
      <div className="k">{name}</div>
      <div className="v">{value}</div>
      <div className="d">{note}</div>
    </div>
  );
}

function Figures({
  detail,
  effort,
  now,
}: {
  readonly detail: TicketDetail;
  readonly effort: Remote<ReviewEffort>;
  readonly now: number;
}) {
  const runs = detail.workloads;
  const running = runs.filter((run) => isActive(run.state)).length;
  const first = runs[0];
  const live = runs.some((run) => run.finishedAt === null);
  const last = Math.max(...runs.map((run) => Date.parse(run.finishedAt ?? run.createdAt)));
  const elapsed = first === undefined ? null : (live ? now : last) - Date.parse(first.createdAt);
  const usd = detail.spend.totals["usd"];
  const tokens = Object.entries(detail.spend.totals).filter(([unit]) => unit !== "usd");
  const found = effort.kind === "loaded" ? effort.value.lenses.reduce((n, l) => n + l.found, 0) : 0;
  const fixed = effort.kind === "loaded" ? effort.value.lenses.reduce((n, l) => n + l.resolved, 0) : 0;

  return (
    <div className="tiles" aria-label="Figures">
      <Tile
        name="Spend"
        value={usd === undefined ? "none" : amount("usd", usd)}
        note={tokens.length === 0 ? "no tokens counted" : tokens.map(([unit, value]) => amount(unit, value)).join(" and ")}
      />
      <Tile
        name="Runs"
        value={String(runs.length)}
        note={running > 0 ? `${running} running` : runs.length === 0 ? "none yet" : "none running"}
      />
      <Tile
        name="Review rounds"
        value={effort.kind === "loaded" ? String(reviewRounds(effort.value)) : "-"}
        note={
          effort.kind !== "loaded"
            ? "not read yet"
            : `${found} ${found === 1 ? "finding" : "findings"}, ${fixed} fixed`
        }
      />
      <Tile
        name="Elapsed"
        value={elapsed === null || Number.isNaN(elapsed) ? "-" : span(elapsed)}
        note={live ? "since approval, still going" : "approval to the last run's end"}
      />
    </div>
  );
}

function Header({ detail, now }: { readonly detail: TicketDetail; readonly now: number }) {
  const first = detail.workloads[0];
  const approver = detail.workloads.find((run) => run.approvedBy !== null)?.approvedBy ?? null;
  const pull = detail.pullRequest;
  return (
    <div className="head">
      <div>
        <h2>{detail.title ?? `Ticket #${detail.number}`}</h2>
        <p className="sub">
          {`${detail.repository} #${detail.number}`}
          {approver !== null && ` · approved by ${approver}`}
          {first !== undefined && ` · ${approver === null ? "approved " : ""}${ago(first.createdAt, now)}`}
        </p>
      </div>
      <div className="actions">
        {detail.url !== null && (
          <a className="chip" href={detail.url}>
            On GitHub ↗
          </a>
        )}
        {pull !== null && (
          <a className="chip pull-request" href={pull.url} rel="noreferrer noopener">
            <span
              className="checks-dot"
              data-checks={pull.checks}
              role="img"
              aria-label={`checks ${pull.checks}, pull request ${pull.state}`}
            />
            {`PR #${pull.number} · checks ${pull.checks} ↗`}
          </a>
        )}
      </div>
    </div>
  );
}

function WhereItIs({ detail, settled }: { readonly detail: TicketDetail; readonly settled: boolean }) {
  const track = trackOf({ workloads: detail.workloads, pullRequest: detail.pullRequest, settled });
  // "Done" is the ticket's word, and the track says it only when every step is.
  const done = track.steps.every((step) => step.state === "done");
  return (
    <section className="panel" aria-labelledby="where-heading">
      <h3 id="where-heading">Where it is</h3>
      <div className="big-track" aria-hidden="true">
        {track.steps.map((step) => (
          <div key={step.id} data-state={step.state}>
            <i />
            {step.label}
          </div>
        ))}
      </div>
      <p className="where-words">
        <b>{done ? "Done" : stageWords(track)}</b>
      </p>
    </section>
  );
}

/** What happened at the gates, as one line: how many of each outcome, and why the first one that did not pass did not. */
function gateLine(gates: readonly Gate[]): string {
  const counts = new Map<string, number>();
  for (const gate of gates) {
    const outcome = gate.outcome ?? "not recorded";
    counts.set(outcome, (counts.get(outcome) ?? 0) + 1);
  }
  const parts = [...counts].map(([outcome, n]) => `${n} ${outcome}`);
  const noted = gates.find((gate) => gate.outcome !== "passed" && gate.reason !== null);
  return `${gates.length} ${gates.length === 1 ? "gate" : "gates"}: ${parts.join(", ")}${
    noted === undefined ? "" : `. ${noted.name}: ${noted.reason}`
  }`;
}

/** The gates of the latest run that kept any, as chips with a line under them saying what happened. */
function GatesPanel({ client, runs }: { readonly client: Client; readonly runs: readonly Run[] }) {
  const ids = runs.map((run) => run.id).join(",");
  const kept = useRemote(async () => {
    const all = await Promise.all(runs.map((run) => client.gates(run.id)));
    return all.filter((gates) => gates.length > 0).at(-1) ?? null;
  }, [client, ids]);
  return (
    <section className="panel" aria-labelledby="gates-heading">
      <h3 id="gates-heading">Gates</h3>
      <Shown state={kept}>
        {(gates) =>
          gates === null ? (
            <p>No run has kept a gate.</p>
          ) : (
            <>
              <div className="actions" role="list" aria-label="Gates">
                {gates.map((gate) => (
                  <span className="chip" role="listitem" key={gate.name}>
                    <span className="state" data-state={gate.outcome ?? "none"}>
                      {gate.outcome ?? "-"}
                    </span>
                    {gate.name}
                  </span>
                ))}
              </div>
              <p className="small">{gateLine(gates)}</p>
            </>
          )
        }
      </Shown>
    </section>
  );
}

function RunsPanel({ detail, now }: { readonly detail: TicketDetail; readonly now: number }) {
  const { lanes, axis } = timelineOf(detail.workloads, now, detail.pullRequest?.state === "merged");
  return (
    <section className="panel" aria-labelledby="runs-heading">
      <h3 id="runs-heading">Runs</h3>
      <p className="small">A run is one attempt by an agent. The ticket's stage is above.</p>
      <div className="lanes" role="list">
        {lanes.map((lane) => (
          <div className="lane" role="listitem" key={lane.id} data-lane={lane.id}>
            <span className="lbl">
              <Link to={runPath(lane.id)}>{lane.label}</Link>
            </span>
            <div className="rail">
              {lane.gap !== null && (
                <span className="brk" style={{ left: `${lane.gap.left}%` }}>
                  {lane.gap.text}
                </span>
              )}
              <span
                className="bar"
                data-tone={lane.state.tone}
                style={{ left: `${lane.left}%`, width: `${lane.width}%` }}
              />
            </div>
            <span className="small" title={lane.state.meaning}>
              {lane.duration} · {lane.state.word}
            </span>
          </div>
        ))}
      </div>
      <div className="axis" aria-hidden="true">
        <div />
        <div className="ticks">
          {axis.ticks.map((tick) => (
            <span key={tick.label} style={{ left: `${tick.left}%` }}>
              {tick.label}
            </span>
          ))}
        </div>
        <div />
      </div>
      <TableScroll>
        <table aria-label="Runs in a table">
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
                  <span className="state" data-state={run.state} title={runWord(run.state).meaning}>
                    {runWord(run.state).word}
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
      </TableScroll>
    </section>
  );
}

export function TicketDetailPage({
  client,
  repository,
  number,
  now = Date.now(),
}: {
  readonly client: Client;
  readonly repository: string;
  readonly number: number;
  /** The time the page measures ages and running runs against. */
  readonly now?: number;
}) {
  const ticket = useRemote(() => client.ticket(repository, number), [client, repository, number]);
  const verdicts = useRemote(() => client.verdicts(repository, number), [client, repository, number]);
  const effort = useRemote(() => client.reviewEffort(repository, number), [client, repository, number]);
  // A ticket is judged when its latest verdict says every requirement is met.
  const settled = verdicts.kind === "loaded" && verdicts.value.at(-1)?.verdict === "met";

  return (
    <Shown state={ticket}>
      {(detail) =>
        detail === null ? (
          <p>No run has been made for this ticket.</p>
        ) : (
          <>
            <Header detail={detail} now={now} />
            <WhereItIs detail={detail} settled={settled} />
            <Figures detail={detail} effort={effort} now={now} />

            <QuestionPanel client={client} repository={repository} number={number} />

            <RunsPanel detail={detail} now={now} />

            <GatesPanel client={client} runs={detail.workloads} />

            <section className="panel" aria-labelledby="words-heading">
              <h3 id="words-heading">What the agent said</h3>
              {detail.workloads.map((run) => (
                <RunWords key={run.id} client={client} run={run} />
              ))}
            </section>

            <section className="panel" aria-labelledby="verdicts-heading">
              <h3 id="verdicts-heading">Verdict history</h3>
              <Shown state={verdicts}>
                {(all) => <VerdictHistory verdicts={all} />}
              </Shown>
            </section>

            <section className="panel" aria-labelledby="effort-heading">
              <h3 id="effort-heading">Review effort</h3>
              <Shown state={effort}>
                {(value) => <ReviewEffortPanel effort={value} />}
              </Shown>
            </section>

            <section className="panel" aria-labelledby="spend-heading">
              <h3 id="spend-heading">Spend</h3>
              <p className="figure">{amounts(detail.spend.totals)}</p>
              <dl className="spend">
                {Object.entries(detail.spend.byKind).map(([kind, totals]) => (
                  <div key={kind}>
                    <dt>{kind}</dt>
                    <dd className="figure">{Object.entries(totals).map(([unit, value]) => amount(unit, value)).join(" and ")}</dd>
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
