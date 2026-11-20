"use client";

import { useCallback, useEffect, useSyncExternalStore } from "react";
import { useThreadView } from "@/features/chat/components/thread-view";
import { progressWords } from "@/features/chat/lib/a2ui/status";

/** How long the plan stays quiet after a change before it is announced: a burst of updates is one. */
export const ANNOUNCE_AFTER_MS = 800;
/** The longest a change waits for that quiet: a plan updated more often than that is still announced. */
export const ANNOUNCE_MAX_WAIT_MS = 5000;
/**
 * A plan that was drawn less than this ago is still being populated (a transcript that replays, or one that has not
 * switched to the thread yet): what changes in that time is the plan arriving, not the plan changing.
 */
export const ANNOUNCE_SETTLE_MS = 1000;
/** How long what the page remembers of a plan outlives the last component that showed it. */
const FORGET_AFTER_MS = 5000;

type Progress = { done: number; total: number } | undefined;
type Entry = {
  /** The plan as it was last drawn: an announcement is for a change from this. */
  seen: string | undefined;
  progress: Progress;
  /** How many times it has been said; a new number is a new node in the region, which is read again. */
  n: number;
  text: string;
  timer: ReturnType<typeof setTimeout> | undefined;
  /** When the change now waiting first came (the maximum wait counts from it). */
  since: number;
  /** When the page first drew this plan. */
  born: number;
  listeners: Set<() => void>;
};

/**
 * What the page remembers of each plan, outside any component: the message that draws a plan is drawn
 * again when its run ends (the live copy is replaced by the log's), and a timer or a "seen" that lived in
 * the component would be lost with it, and with it the announcement. Keyed by the thread AND the plan's
 * surface id (docs/api/ui-catalog-v1.md, "The plan"): an activity key is only unique in a thread, and
 * nothing has to run in some order when the person moves to another thread.
 */
const entries = new Map<string, Entry>();
const scope = (threadId: string | null | undefined, surfaceId: string) =>
  `${threadId ?? ""}#${surfaceId}`;

/** Forgets every plan (tests: each starts from a page that has seen nothing). */
export function forgetPlans(): void {
  for (const e of entries.values()) clearTimeout(e.timer);
  entries.clear();
}

/** What the page remembers of a plan, made when it is first drawn (pure: nothing is said or timed here). */
function entryOf(id: string): Entry {
  let e = entries.get(id);
  if (!e) {
    e = {
      seen: undefined,
      progress: undefined,
      n: 0,
      text: "",
      timer: undefined,
      since: 0,
      born: Date.now(),
      listeners: new Set(),
    };
    entries.set(id, e);
  }
  return e;
}

/** The plan was drawn as `signature`: the first time that is only noted; a change is said once it has been quiet. */
function track(entry: Entry, signature: string, progress: Progress): void {
  if (entry.seen === undefined) {
    entry.seen = signature; // the first copy is read where it stands, not announced
    entry.progress = progress;
    return;
  }
  if (entry.seen === signature) return;
  entry.seen = signature;
  entry.progress = progress;
  const now = Date.now();
  if (now - entry.born < ANNOUNCE_SETTLE_MS) return;
  if (entry.timer === undefined) entry.since = now;
  clearTimeout(entry.timer);
  const wait = Math.max(0, Math.min(ANNOUNCE_AFTER_MS, entry.since + ANNOUNCE_MAX_WAIT_MS - now));
  entry.timer = setTimeout(() => {
    entry.timer = undefined;
    entry.n += 1;
    entry.text = entry.progress ? `Plan updated: ${progressWords(entry.progress)}` : "Plan updated";
    for (const l of entry.listeners) l();
  }, wait);
}

/**
 * Says, to assistive technology and politely, that the plan changed ("Plan updated: 2 of 5 done"). It is
 * a status region that is always in the page, so a change is read without moving the focus. What it
 * reads is the plan's own state, never the agent's words: a reply that streams does not touch it. The
 * first copy of the plan is not announced (the person reads it where it stands), and a burst of
 * updates is announced once, after the plan has been quiet for {@link ANNOUNCE_AFTER_MS} and at the latest
 * {@link ANNOUNCE_MAX_WAIT_MS} after the burst began (docs/api/ui-catalog-v1.md, "The plan"). `signature` is
 * the plan as drawn: it changes when the content does; `surfaceId` names the plan.
 */
export function PlanAnnouncer({
  surfaceId,
  signature,
  progress,
}: {
  surfaceId: string;
  signature: string;
  progress: Progress;
}) {
  const { threadId } = useThreadView();
  const id = scope(threadId, surfaceId);
  const entry = entryOf(id);
  const subscribe = useCallback(
    (listener: () => void) => {
      entry.listeners.add(listener);
      return () => {
        entry.listeners.delete(listener);
        // nobody draws this plan any more: after a while the page forgets it was ever seen
        setTimeout(() => {
          if (
            entry.listeners.size === 0 &&
            entry.timer === undefined &&
            entries.get(id) === entry
          ) {
            entries.delete(id);
          }
        }, FORGET_AFTER_MS);
      };
    },
    [entry, id],
  );
  const n = useSyncExternalStore(
    subscribe,
    () => entry.n,
    () => 0,
  );
  useEffect(() => {
    track(entryOf(id), signature, progress);
  }, [id, signature, progress]);
  return (
    <div role="status" aria-live="polite" data-slot="plan-status" className="sr-only">
      {n > 0 ? <p key={n}>{entry.text}</p> : null}
    </div>
  );
}
