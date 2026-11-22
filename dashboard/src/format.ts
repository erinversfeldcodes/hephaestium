import type { Totals } from "./api";

const counts = new Intl.NumberFormat("en-US");

/** One amount in its own unit. */
export function amount(unit: string, value: number): string {
  if (unit === "usd") return `$${value.toFixed(2)}`;
  return `${counts.format(value)} ${unit}`;
}

/** Each unit on its own, joined: dollars and tokens are never added to each other. */
export function amounts(totals: Totals): string {
  const parts = Object.entries(totals).map(([unit, value]) => amount(unit, value));
  return parts.length === 0 ? "nothing" : parts.join(" and ");
}

/** A time as the person's browser writes it, or a dash for none. */
export function when(time: string | null): string {
  if (time === null) return "-";
  const date = new Date(time);
  return Number.isNaN(date.getTime()) ? time : date.toLocaleString();
}
