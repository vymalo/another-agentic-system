/**
 * What the status components of the UI catalog (docs/api/ui-catalog-v1.md, version 6, group 2) show: the
 * states a checklist item and a timeline event can be in, what each says in words, and how far a checklist
 * has got. Pure: no React, no I/O. A state is always said in words beside its icon, never by colour alone.
 */

export type ItemState = "done" | "doing" | "todo" | "failed";
/** A checklist item may also be dropped (catalog version 8): given up, no longer part of the plan. */
export type ChecklistState = ItemState | "dropped";

/** What a state says, in words: the text beside its icon, and what a screen reader reads. */
export const STATE_WORDS: Record<ChecklistState, string> = {
  done: "Done",
  doing: "In progress",
  todo: "To do",
  failed: "Failed",
  dropped: "Dropped",
};

export type ChecklistItem = { label: string; state: ChecklistState; note?: string };
export type TimelineEvent = {
  time: string;
  title: string;
  description?: string;
  state: ItemState;
};

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);
const isChecklistState = (v: unknown): v is ChecklistState =>
  typeof v === "string" && Object.hasOwn(STATE_WORDS, v);
/** A timeline's states are the four of version 6: only a checklist can drop an item. */
const isState = (v: unknown): v is ItemState => isChecklistState(v) && v !== "dropped";

/** The items of a checklist that passed the schema. A state that is not one of the four is to do. */
export function readItems(v: unknown): ChecklistItem[] {
  if (!Array.isArray(v)) return [];
  const out: ChecklistItem[] = [];
  for (const item of v) {
    if (!isRecord(item)) continue;
    const label = str(item.label);
    if (label === undefined) continue;
    const note = str(item.note);
    out.push({
      label,
      state: isChecklistState(item.state) ? item.state : "todo",
      ...(note ? { note } : {}),
    });
  }
  return out;
}

/** The events of a timeline that passed the schema. A missing state is done (the usual case). */
export function readEvents(v: unknown): TimelineEvent[] {
  if (!Array.isArray(v)) return [];
  const out: TimelineEvent[] = [];
  for (const event of v) {
    if (!isRecord(event)) continue;
    const time = str(event.time);
    const title = str(event.title);
    if (time === undefined || title === undefined) continue;
    const description = str(event.description);
    out.push({
      time,
      title,
      ...(description ? { description } : {}),
      state: isState(event.state) ? event.state : "done",
    });
  }
  return out;
}

/**
 * How many items are done, out of the items that are still part of the plan, and the share as a whole
 * percentage (0 with none). A dropped item is not part of the plan: it is in neither number
 * (docs/api/ui-catalog-v1.md, "Version 8").
 */
export function progressOf(items: readonly { state: ChecklistState }[]): {
  done: number;
  total: number;
  percent: number;
} {
  const kept = items.filter((i) => i.state !== "dropped");
  const total = kept.length;
  const done = kept.filter((i) => i.state === "done").length;
  return { done, total, percent: total === 0 ? 0 : Math.round((done * 100) / total) };
}

/** What a progress says in words, for the count beside the bar and for a screen reader: `2 of 5 done`. */
export const progressWords = (p: { done: number; total: number }): string =>
  `${p.done} of ${p.total} done`;
