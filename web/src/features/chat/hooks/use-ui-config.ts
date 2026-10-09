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

function load() {
  if (started) return;
  started = true;
  api
    .GET("/api/config")
    .then(({ data }) => {
      if (!data) throw new Error("no configuration");
      const shown = data.ui?.showDescriptions;
      const history = historyOf(data.ui?.history);
      set({
        loaded: true,
        config: {
          showDescriptions: typeof shown === "boolean" ? shown : DEFAULT_UI_CONFIG.showDescriptions,
          ...(history ? { history } : {}),
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

/** The public configuration the web follows; `loaded` is false until the first answer. */
export function useUiConfig(): State {
  return useSyncExternalStore(
    subscribe,
    () => state,
    () => SERVER,
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
