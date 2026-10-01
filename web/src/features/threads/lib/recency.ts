/** The groups of the thread list, newest first (web/DESIGN.md, "Layout"). */
export const RECENCY = [
  "Today",
  "Yesterday",
  "Previous 7 days",
  "Previous 30 days",
  "Older",
] as const;
export type Recency = (typeof RECENCY)[number];

const DAY = 24 * 60 * 60 * 1000;

/** Midnight of the local day `t` falls in. */
const startOfDay = (t: Date): number =>
  new Date(t.getFullYear(), t.getMonth(), t.getDate()).getTime();

/** Which group a thread last touched at `at` goes in, seen at `now` (local days). */
export function recencyOf(at: string, now: Date): Recency {
  const t = Date.parse(at);
  if (Number.isNaN(t)) return "Older";
  const today = startOfDay(now);
  if (t >= today) return "Today";
  if (t >= today - DAY) return "Yesterday";
  if (t >= today - 7 * DAY) return "Previous 7 days";
  if (t >= today - 30 * DAY) return "Previous 30 days";
  return "Older";
}

/**
 * The items in their groups, in the order they came (the list is newest first), without the empty
 * groups. A thread from the future (a skewed clock) is "Today".
 */
export function groupByRecency<T extends { updatedAt: string }>(
  items: readonly T[],
  now: Date,
): { group: Recency; items: T[] }[] {
  const groups = new Map<Recency, T[]>();
  for (const item of items) {
    const group = recencyOf(item.updatedAt, now);
    const list = groups.get(group) ?? [];
    list.push(item);
    groups.set(group, list);
  }
  return RECENCY.flatMap((group) => {
    const list = groups.get(group);
    return list ? [{ group, items: list }] : [];
  });
}
