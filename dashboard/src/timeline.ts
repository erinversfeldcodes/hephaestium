import type { Run } from "./api";
import { runWord, type RunWord } from "./runwords";

/** A wait between runs longer than this is drawn as a break and not to scale. */
export const BREAK_AFTER_MS = 60 * 60 * 1000;
/** How much of the axis a break takes, as a share of the time the runs themselves took. */
const BREAK_SHARE = 0.12;
/** A bar is never thinner than this, as a percentage, so a run of a second is still seen. */
const MIN_WIDTH = 0.6;
const MINUTE = 60 * 1000;
const steps = [1, 2, 5, 10, 15, 20, 30, 60, 120, 180, 240, 360, 720, 1440].map((m) => m * MINUTE);

export interface Lane {
  readonly id: string;
  readonly label: string;
  readonly state: RunWord;
  /** Where the bar starts and how wide it is, as percentages of the axis. */
  readonly left: number;
  readonly width: number;
  /** "41 min". */
  readonly duration: string;
  /** The wait before this run that is not drawn to scale, or null. */
  readonly gap: { readonly left: number; readonly text: string } | null;
}

export interface Axis {
  readonly ticks: readonly { readonly left: number; readonly label: string }[];
}

export interface Timeline {
  readonly lanes: readonly Lane[];
  readonly axis: Axis;
}

/** "41 min", "1 h 5 m", "under a minute". */
export function span(ms: number): string {
  const minutes = Math.round(ms / MINUTE);
  if (ms < MINUTE / 2) return ms < 1000 ? "under a second" : `${Math.round(ms / 1000)} s`;
  if (minutes < 60) return `${minutes} min`;
  const rest = minutes % 60;
  return rest === 0 ? `${Math.floor(minutes / 60)} h` : `${Math.floor(minutes / 60)} h ${rest} m`;
}

/** A tick's label: round minutes under an hour, hours above. */
function tickLabel(ms: number): string {
  const minutes = ms / MINUTE;
  if (minutes === 0) return "0";
  if (minutes < 60) return `${minutes} min`;
  return minutes % 60 === 0 ? `${minutes / 60} h` : `${Math.floor(minutes / 60)} h ${minutes % 60} m`;
}

function laneLabel(run: Run, lens: string | null | undefined): string {
  if (run.kind === "implementation") return "Implementation";
  if (run.kind === "review") return lens ? `Review · ${lens}` : "Review";
  if (run.kind === "completeness" || run.kind === "gap") return "Judging";
  return run.kind.charAt(0).toUpperCase() + run.kind.slice(1);
}

interface Placed {
  readonly run: Run;
  readonly start: number;
  readonly end: number;
}

/**
 * One lane per run on one time axis. Each run starts when it started (when it was made if it has
 * not started) and ends when it finished, or at `now` while it goes on. A wait of more than an
 * hour between one run and the next is cut out and replaced by a break of a fixed width, so the
 * runs either side stay readable; the axis ticks are drawn only where the scale is true, before
 * the first break.
 */
export function timelineOf(
  runs: readonly (Run & { readonly lens?: string | null })[],
  now: number,
  merged: boolean,
): Timeline {
  const placed: (Placed & { readonly lens: string | null })[] = runs
    .map((run) => {
      const start = Date.parse(run.startedAt ?? run.createdAt);
      // A run that has not started has no length; one that has started and not finished goes on until now.
      const end = run.finishedAt !== null ? Date.parse(run.finishedAt) : run.startedAt === null ? start : now;
      return { run, lens: run.lens ?? null, start, end: Math.max(start, end) };
    })
    .filter((p) => !Number.isNaN(p.start))
    .sort((a, b) => a.start - b.start);
  if (placed.length === 0) return { lanes: [], axis: { ticks: [] } };

  // The compressed position of each run, the waits that are cut out, and the breaks' widths.
  const t0 = placed[0]!.start;
  let cut = 0;
  let reached = placed[0]!.end;
  const pieces = placed.map((p, index) => {
    let gap: { at: number; ms: number } | null = null;
    if (index > 0 && p.start - reached > BREAK_AFTER_MS) {
      gap = { at: reached - cut - t0, ms: p.start - reached };
      cut += p.start - reached;
    }
    reached = Math.max(reached, p.end);
    return { p, gap, from: p.start - cut - t0 };
  });
  const realSpan = reached - t0 - cut;
  const breakWidth = Math.max(realSpan, MINUTE) * BREAK_SHARE;
  const breaks = pieces.filter((x) => x.gap !== null).length;
  const total = Math.max(realSpan + breaks * breakWidth, MINUTE);

  const step = steps.find((s) => total / s <= 5) ?? steps[steps.length - 1]!;
  const max = Math.ceil(total / step) * step;

  // Time before break k is shifted right by the k breaks' widths already drawn.
  let drawn = 0;
  let firstBreak: number | null = null;
  const lanes = pieces.map(({ p, gap, from }): Lane => {
    let gapMark: Lane["gap"] = null;
    if (gap !== null) {
      const left = ((gap.at + drawn * breakWidth + breakWidth / 2) / max) * 100;
      firstBreak ??= gap.at;
      drawn += 1;
      gapMark = { left, text: `${span(gap.ms)} later${merged ? ", after the merge" : ""}` };
    }
    const left = ((from + drawn * breakWidth) / max) * 100;
    const width = Math.max(((p.end - p.start) / max) * 100, MIN_WIDTH);
    return {
      id: p.run.id,
      label: laneLabel(p.run, p.lens),
      state: runWord(p.run.state),
      left,
      width: Math.min(width, 100 - left),
      duration: p.run.startedAt === null && p.run.finishedAt === null ? "not started" : span(p.end - p.start),
      gap: gapMark,
    };
  });

  const trueUntil = firstBreak ?? max;
  const ticks: { left: number; label: string }[] = [];
  for (let at = 0; at <= max && at <= trueUntil; at += step) ticks.push({ left: (at / max) * 100, label: tickLabel(at) });
  return { lanes, axis: { ticks } };
}
