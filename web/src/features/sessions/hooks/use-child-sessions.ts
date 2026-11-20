import { useCallback, useMemo, useState } from "react";
import { useThreadView } from "@/features/chat/components/thread-view";
import type { ApiThread } from "@/lib/api/types";
import {
  type AppliedClose,
  type MergedChildren,
  mergeChildren,
  startsWaiting,
} from "../lib/children";

export type ChildSessions = MergedChildren & {
  /** Subagents open now (working or idle, a closing one too): what `maxOpen` counts. */
  openCount: number;
  /** Starts that wait for their outcome: at most 4 may. */
  waiting: number;
  /** The answer of a close, applied at once, before the rail says it. */
  apply: (child: string, state: AppliedClose) => void;
  /** What those closes said, by child thread: a subagent's own thread reads its own. */
  applied: ReadonlyMap<string, AppliedClose>;
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
  const { sessions, sessionStarts } = useThreadView();
  const [applied, setApplied] = useState<ReadonlyMap<string, AppliedClose>>(new Map());
  const apply = useCallback(
    (child: string, state: AppliedClose) => setApplied((m) => new Map(m).set(child, state)),
    [],
  );
  return useMemo(() => {
    const starts = sessionStarts ?? new Map();
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
    };
  }, [parent, rows, sessions, sessionStarts, applied, local, apply]);
}
