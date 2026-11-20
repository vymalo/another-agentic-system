import { useCallback, useMemo, useRef, useState } from "react";
import { useThreadView } from "@/features/chat/components/thread-view";
import type { ApiThread } from "@/lib/api/types";
import {
  type AppliedClose,
  type AppliedEntry,
  type Arrivals,
  arrivalsOf,
  liveCloses,
  type MergedChildren,
  mergeChildren,
  NO_ARRIVALS,
  noteArrivals,
  startsWaiting,
} from "../lib/children";

export type ChildSessions = MergedChildren & {
  /** Subagents open now (working or idle, a closing one too): what `maxOpen` counts. */
  openCount: number;
  /** Starts that wait for their outcome: at most 4 may. */
  waiting: number;
  /** The answer of a close, applied at once, before the rail says it; `cancel` says it stopped the round. */
  apply: (child: string, state: AppliedClose, cancel?: boolean) => void;
  /** What those closes said, by child thread: a subagent's own thread reads its own. A newer row or card ends one. */
  applied: ReadonlyMap<string, AppliedClose>;
  /** The children whose round a close of this page stopped, still on its way: nothing is stronger. */
  forced: ReadonlySet<string>;
};

/**
 * The subagents of the open thread: the rail's rows (the truth), the stream's cards (a session whose
 * rail page is not loaded) and the starts that wait, merged by `mergeChildren`. `local` are the requests
 * this page sent that the stream has not said yet, so that a second start cannot slip past the limit of
 * four while the first one's card is on its way.
 */
export function useChildSessions(
  parent: string,
  rows: readonly ApiThread[],
  local: readonly string[],
): ChildSessions {
  const { sessions, sessionStarts, self } = useThreadView();
  const [asked, setAsked] = useState<ReadonlyMap<string, AppliedEntry>>(new Map());
  // what has arrived about each child, counted in the order it arrived: a close is over once something
  // newer than its answer arrives, whatever either clock says
  const arrivals = useRef<Arrivals>(NO_ARRIVALS);
  const apply = useCallback((child: string, state: AppliedClose, cancel = false) => {
    const after = arrivalsOf(arrivals.current, child);
    setAsked((m) => new Map(m).set(child, { state, cancel, after }));
  }, []);
  return useMemo(() => {
    const starts = sessionStarts ?? new Map();
    arrivals.current = noteArrivals(arrivals.current, {
      thread: parent,
      rows,
      cards: sessions ?? new Map(),
      self: self ?? null,
    });
    const live = liveCloses(asked, arrivals.current);
    const applied = new Map([...live].map(([child, e]) => [child, e.state] as const));
    const forced = new Set([...live].filter(([, e]) => e.cancel).map(([child]) => child));
    const merged = mergeChildren({
      parent,
      rows,
      cards: sessions ?? new Map(),
      starts,
      applied,
    });
    return {
      ...merged,
      openCount: merged.open.length,
      waiting: startsWaiting(starts, local),
      apply,
      applied,
      forced,
    };
  }, [parent, rows, sessions, sessionStarts, self, asked, local, apply]);
}
