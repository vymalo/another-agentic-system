import { useCallback, useSyncExternalStore } from "react";
import { useMe } from "@/features/me/hooks/use-me";
import { isAdmin } from "@/features/me/lib/access";

/** Whose threads the sidebar lists: the person's own, or (an administrator) everyone's. */
export type ThreadScope = "mine" | "all";

const KEY = "another-agentic.thread-scope";

/**
 * A page of the app is a page load of its own to the shell (every navigation remounts it), so the
 * choice lives here, outside the components: in memory for the pages a client navigation moves
 * between, and in the browser's storage for the next visit. Storage may be blocked: the choice is
 * then only kept while the tab is.
 */
let scope: ThreadScope | null = null;
const listeners = new Set<() => void>();

function read(): ThreadScope {
  if (scope === null) {
    try {
      scope = window.localStorage.getItem(KEY) === "all" ? "all" : "mine";
    } catch {
      scope = "mine";
    }
  }
  return scope;
}

function write(next: ThreadScope) {
  scope = next;
  try {
    window.localStorage.setItem(KEY, next);
  } catch {
    // not remembered for the next visit
  }
  for (const listener of listeners) listener();
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

export type ThreadScopeView = {
  /** The person may list everyone's threads (`admin`, and `thread.read` of scope `any`). */
  canSeeAll: boolean;
  /** What the list shows: "all" only for someone who can see all, whatever was chosen before. */
  scope: ThreadScope;
  setScope: (next: ThreadScope) => void;
};

export function useThreadScope(): ThreadScopeView {
  const { me } = useMe();
  const chosen = useSyncExternalStore(subscribe, read, () => "mine" as const);
  const canSeeAll = me !== null && isAdmin(me);
  const setScope = useCallback((next: ThreadScope) => write(next), []);
  return { canSeeAll, scope: canSeeAll ? chosen : "mine", setScope };
}

/** Forgets the choice, for tests. */
export function resetThreadScope() {
  scope = null;
  try {
    window.localStorage.removeItem(KEY);
  } catch {
    // nothing to forget
  }
}
