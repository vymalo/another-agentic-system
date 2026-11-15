import { useSyncExternalStore } from "react";
import { api } from "@/lib/api/client";

/** The `ui` section of `GET /api/config` (ADR 0034), every key with the default a missing one has. */
export type UiConfig = {
  /** Whether a thread's description is shown (ADR 0035). The API returns it either way. */
  showDescriptions: boolean;
  /**
   * How a long thread is opened (ADR 0059). **Present only when the orchestrator serves the history route**: an older one, or a
   * configuration it does not know, leaves it out, and the thread is replayed from its first event.
   */
  history?: {
    initialTurns: number;
    pageTurns: number;
    maxTurns: number;
    /** The version of the frames the route writes (`projection` of a page). */
    projection: number;
    /** Whether to open a thread from its history; false is a replay. */
    windowed: boolean;
  };
  /**
   * `search` of `GET /api/config` (ADR 0068), which is not under `ui`: **its presence is the capability**, so an orchestrator
   * that does not serve `searchThreads` leaves it out and the web draws no search box. The numbers are what the server enforces.
   */
  search?: { maxQueryLength: number; maxTerms: number };
};

export const DEFAULT_UI_CONFIG: UiConfig = { showDescriptions: true };

type State = { loaded: boolean; config: UiConfig };

/**
 * The configuration changes only when the orchestrator restarts, so it is read once per page load
 * and every component that asks shares the answer. Until it is known nothing it can hide is drawn
 * (a description that is then taken away would flash); a configuration that cannot be read leaves
 * the defaults, which is what a client does with a key it does not know, and is tried again by
 * the next component that asks.
 */
let state: State = { loaded: false, config: DEFAULT_UI_CONFIG };
let started = false;
const listeners = new Set<() => void>();

function set(next: State) {
  state = next;
  for (const listener of listeners) listener();
}

/** `ui.history`, if it says what a client needs to open a thread from it; else none (the log is replayed). */
function historyOf(value: unknown): UiConfig["history"] | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const h = value as Record<string, unknown>;
  const count = (v: unknown): number | undefined =>
    typeof v === "number" && Number.isInteger(v) && v >= 1 ? v : undefined;
  const initialTurns = count(h.initialTurns);
  const pageTurns = count(h.pageTurns);
  const maxTurns = count(h.maxTurns);
  const projection = count(h.projection);
  if (!initialTurns || !pageTurns || !maxTurns || !projection) return undefined;
  return { initialTurns, pageTurns, maxTurns, projection, windowed: h.windowed === true };
}

/** `search` of the configuration, if it names both limits as positive whole numbers; else none (no box). */
export function searchOf(value: unknown): UiConfig["search"] | undefined {
  if (typeof value !== "object" || value === null) return undefined;
  const s = value as Record<string, unknown>;
  const count = (v: unknown): number | undefined =>
    typeof v === "number" && Number.isInteger(v) && v >= 1 ? v : undefined;
  const maxQueryLength = count(s.maxQueryLength);
  const maxTerms = count(s.maxTerms);
  return maxQueryLength && maxTerms ? { maxQueryLength, maxTerms } : undefined;
}

function load() {
  if (started) return;
  started = true;
  api
    .GET("/api/config")
    .then(({ data }) => {
      if (!data) throw new Error("no configuration");
      const shown = data.ui?.showDescriptions;
      const history = historyOf(data.ui?.history);
      const search = searchOf(data.search);
      set({
        loaded: true,
        config: {
          showDescriptions: typeof shown === "boolean" ? shown : DEFAULT_UI_CONFIG.showDescriptions,
          ...(history ? { history } : {}),
          ...(search ? { search } : {}),
        },
      });
    })
    .catch(() => {
      started = false;
      set({ loaded: true, config: DEFAULT_UI_CONFIG });
    });
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  load();
  return () => {
    listeners.delete(listener);
  };
};

const SERVER: State = { loaded: false, config: DEFAULT_UI_CONFIG };

/** What a page that asks nothing knows: the defaults, at once. */
const UNASKED: State = { loaded: true, config: DEFAULT_UI_CONFIG };
const unasked = () => () => {};

/**
 * The public configuration the web follows; `loaded` is false until the first answer. With `asked` false it is not read at all
 * and is the defaults at once: `GET /api/config` is behind the identity layer, and the signed-in client holds a request that
 * is refused with a 401 for a sign-in (`withSessionRefresh`), which a reader of a public link, who has none, must never wait for.
 */
export function useUiConfig(asked = true): State {
  return useSyncExternalStore(
    asked ? subscribe : unasked,
    () => (asked ? state : UNASKED),
    () => (asked ? SERVER : UNASKED),
  );
}

/** Whether descriptions are drawn: once the configuration is known, and it does not hide them. */
export function useShowDescriptions(): boolean {
  const { loaded, config } = useUiConfig();
  return loaded && config.showDescriptions;
}

/** Forgets what was read, for tests. */
export function resetUiConfig() {
  started = false;
  state = { loaded: false, config: DEFAULT_UI_CONFIG };
}

/** What the search box needs from the configuration: its limits, or `undefined` while unknown or when the server has no search. */
export function useSearchConfig(): UiConfig["search"] {
  return useUiConfig().config.search;
}
