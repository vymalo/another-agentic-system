import { useSyncExternalStore } from "react";
import { api } from "@/lib/api/client";
import type { ApiMe } from "@/lib/api/types";

/**
 * Who the person is and what their roles let them do (`GET /api/me`, ADR 0033). `unknown`: it could
 * not be read (a 401 the page is leaving for, a network error, an orchestrator from before roles).
 * The screens then hide nothing, as before roles: the orchestrator enforces every request, and its
 * refusals are shown in words where the action was.
 */
export type MeState =
  | { status: "loading"; me: null }
  | { status: "ready"; me: ApiMe }
  | { status: "unknown"; me: null };

const LOADING: MeState = { status: "loading", me: null };

/**
 * Read once per page load, like the public configuration (`use-ui-config.ts`): every component that
 * asks shares the answer, and a read that failed is tried again by the next component that asks.
 */
let state: MeState = LOADING;
let started = false;
const listeners = new Set<() => void>();

function set(next: MeState) {
  state = next;
  for (const listener of listeners) listener();
}

const isMe = (v: unknown): v is ApiMe => {
  if (typeof v !== "object" || v === null) return false;
  const me = v as Partial<ApiMe>;
  return (
    typeof me.user === "string" &&
    Array.isArray(me.roles) &&
    Array.isArray(me.permissions) &&
    typeof me.agents === "object" &&
    me.agents !== null &&
    Array.isArray(me.agents.read) &&
    Array.isArray(me.agents.invoke)
  );
};

function load() {
  if (started) return;
  started = true;
  api
    .GET("/api/me")
    .then(({ data }) => {
      if (!isMe(data)) throw new Error("no identity");
      set({ status: "ready", me: data });
    })
    .catch(() => {
      started = false;
      set({ status: "unknown", me: null });
    });
}

const subscribe = (listener: () => void) => {
  listeners.add(listener);
  load();
  return () => {
    listeners.delete(listener);
  };
};

/** The person, as far as the page knows. */
export function useMe(): MeState {
  return useSyncExternalStore(
    subscribe,
    () => state,
    () => LOADING,
  );
}

/** Forgets what was read, for tests. */
export function resetMe() {
  started = false;
  state = LOADING;
}
