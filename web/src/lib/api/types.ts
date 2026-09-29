import type { components } from "./schema";

export type ApiEvent = components["schemas"]["Event"];
export type ApiThread = components["schemas"]["Thread"];
export type ApiAgent = components["schemas"]["Agent"];
export type ApiActor = components["schemas"]["Actor"];
export type ApiReleases = NonNullable<ApiAgent["releases"]>;
export type ThreadState = components["schemas"]["ThreadState"];
export type EventKind = components["schemas"]["EventKind"];

/** Every kind the contract defines. The guard below fails typecheck when the contract drifts. */
export const EVENT_KINDS = [
  "user_message",
  "agent_message",
  "agent_status",
  "artifact",
  "thread_state",
  "error",
] as const satisfies readonly EventKind[];

type AllKindsListed = Exclude<EventKind, (typeof EVENT_KINDS)[number]> extends never ? true : never;
export const allKindsListed: AllKindsListed = true;

export const THREAD_STATES = [
  "queued",
  "working",
  "blocked",
  "done",
  "failed",
  "cancelled",
] as const satisfies readonly ThreadState[];
type AllStatesListed =
  Exclude<ThreadState, (typeof THREAD_STATES)[number]> extends never ? true : never;
export const allStatesListed: AllStatesListed = true;

/** Mirrors A2A TaskState, as documented on `EventKind.agent_status`. */
export const AGENT_STATUSES = [
  "submitted",
  "working",
  "input_required",
  "completed",
  "failed",
  "canceled",
] as const;
export type AgentStatus = (typeof AGENT_STATUSES)[number];

export type ArtifactData = { name: string; mimeType?: string; uri?: string; text?: string };

type Base = Omit<ApiEvent, "kind" | "data">;
export type TypedEvent = Base &
  (
    | { kind: "user_message"; data: { text: string } }
    | { kind: "agent_message"; data: { text: string; messageId: string; final: boolean } }
    | { kind: "agent_status"; data: { status: AgentStatus; detail?: string } }
    | { kind: "artifact"; data: ArtifactData }
    | { kind: "thread_state"; data: { state: ThreadState } }
    | { kind: "error"; data: { message: string; retryable: boolean } }
  );

const isStr = (v: unknown): v is string => typeof v === "string";
const optStr = (v: unknown): string | undefined => (isStr(v) ? v : undefined);

/**
 * Narrow a contract `Event` (whose `data` is a loose bag) to its per-kind shape.
 * Malformed events are dropped with a warning: the UI renders events, never guesses.
 */
export function toTypedEvent(e: ApiEvent): TypedEvent | null {
  const d = e.data;
  const { kind, ...base } = e;
  const rest: Base = { seq: base.seq, threadId: base.threadId, at: base.at, actor: base.actor };
  switch (kind) {
    case "user_message":
      if (isStr(d.text)) return { ...rest, kind, data: { text: d.text } };
      break;
    case "agent_message":
      if (isStr(d.text) && isStr(d.messageId)) {
        return {
          ...rest,
          kind,
          data: { text: d.text, messageId: d.messageId, final: d.final === true },
        };
      }
      break;
    case "agent_status":
      if (isStr(d.status) && (AGENT_STATUSES as readonly string[]).includes(d.status)) {
        const detail = optStr(d.detail);
        return {
          ...rest,
          kind,
          data: { status: d.status as AgentStatus, ...(detail !== undefined ? { detail } : {}) },
        };
      }
      break;
    case "artifact":
      if (isStr(d.name)) {
        const mimeType = optStr(d.mimeType);
        const uri = optStr(d.uri);
        const text = optStr(d.text);
        return {
          ...rest,
          kind,
          data: {
            name: d.name,
            ...(mimeType !== undefined ? { mimeType } : {}),
            ...(uri !== undefined ? { uri } : {}),
            ...(text !== undefined ? { text } : {}),
          },
        };
      }
      break;
    case "thread_state":
      if (isStr(d.state) && (THREAD_STATES as readonly string[]).includes(d.state)) {
        return { ...rest, kind, data: { state: d.state as ThreadState } };
      }
      break;
    case "error":
      if (isStr(d.message)) {
        return { ...rest, kind, data: { message: d.message, retryable: d.retryable === true } };
      }
      break;
    default: {
      const unknownKind: never = kind;
      console.warn("Dropping event of unknown kind", unknownKind);
      return null;
    }
  }
  console.warn(`Dropping malformed ${kind} event`, e.seq);
  return null;
}

export const TERMINAL_STATES: readonly ThreadState[] = ["done", "failed", "cancelled"];
export const isTerminal = (s: ThreadState | undefined): boolean =>
  s !== undefined && TERMINAL_STATES.includes(s);
export const isActive = (s: ThreadState | undefined): boolean => s === "queued" || s === "working";
