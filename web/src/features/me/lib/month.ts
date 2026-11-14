/**
 * The months of the usage page (ADR 0065): a UTC calendar month is `YYYY-MM`, the shape `getMyUsage` takes
 * and answers. Pure, and never read from the person's zone: the orchestrator's months are UTC ones.
 */

const SHAPE = /^[0-9]{4}-(0[1-9]|1[0-2])$/;

const NAMES = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
] as const;

export const isMonth = (s: string): boolean => SHAPE.test(s);

const pad = (n: number, width: number) => String(n).padStart(width, "0");

/** The UTC month `now` falls in. */
export function currentMonth(now: Date = new Date()): string {
  return `${pad(now.getUTCFullYear(), 4)}-${pad(now.getUTCMonth() + 1, 2)}`;
}

function parts(month: string): [number, number] {
  return [Number(month.slice(0, 4)), Number(month.slice(5, 7))];
}

/** The month `delta` months from `month`; `null` outside the years the API reads (0001 to 9999). */
function shift(month: string, delta: number): string | null {
  const [year, m] = parts(month);
  const index = year * 12 + (m - 1) + delta;
  const y = Math.floor(index / 12);
  if (y < 1 || y > 9999) return null;
  return `${pad(y, 4)}-${pad((index % 12) + 1, 2)}`;
}

export const previousMonth = (month: string): string | null => shift(month, -1);
export const nextMonth = (month: string): string | null => shift(month, 1);

/** `2026-10` as "October 2026". */
export function monthLabel(month: string): string {
  const [year, m] = parts(month);
  return `${NAMES[m - 1]} ${year}`;
}
