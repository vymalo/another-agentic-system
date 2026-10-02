import type { components } from "./schema";

export type ApiThread = components["schemas"]["Thread"];
export type ApiBranches = components["schemas"]["Branches"];
export type ApiBranchPoint = ApiBranches["points"][number];
export type ApiAgent = components["schemas"]["Agent"];
export type ApiMe = components["schemas"]["Me"];
export type ApiPermission = components["schemas"]["Permission"];
export type ApiActor = components["schemas"]["Actor"];
export type ApiReleases = NonNullable<ApiAgent["releases"]>;
export type ThreadState = components["schemas"]["ThreadState"];

export const THREAD_STATES = [
  "queued",
  "working",
  "verifying",
  "blocked",
  "done",
  "failed",
  "cancelled",
] as const satisfies readonly ThreadState[];
type AllStatesListed =
  Exclude<ThreadState, (typeof THREAD_STATES)[number]> extends never ? true : never;
export const allStatesListed: AllStatesListed = true;

/** Mirrors A2A TaskState: the `status` of a `vymalo.status` activity (docs/api/agui.md). */
export const AGENT_STATUSES = [
  "submitted",
  "working",
  "input_required",
  "auth_required",
  "completed",
  "failed",
  "canceled",
] as const;
export type AgentStatus = (typeof AGENT_STATUSES)[number];

export const TERMINAL_STATES: readonly ThreadState[] = ["done", "failed", "cancelled"];
export const isTerminal = (s: ThreadState | undefined): boolean =>
  s !== undefined && TERMINAL_STATES.includes(s);
/** A run is open while the thread is queued, working or (under a gate) being verified. */
export const isActive = (s: ThreadState | undefined): boolean =>
  s === "queued" || s === "working" || s === "verifying";
