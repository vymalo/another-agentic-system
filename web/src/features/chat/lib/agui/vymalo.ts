import { AGENT_STATUSES, type AgentStatus, type ApiActor } from "@/lib/api/types";

/**
 * The `vymalo.*` vocabulary of docs/api/agui.md: activity types, the actor metadata key and the
 * release-channels extension URI. A client that knows none of them still shows conforming runs.
 */
export const ACTIVITY = {
  status: "vymalo.status",
  artifact: "vymalo.artifact",
  error: "vymalo.error",
  action: "vymalo.action",
  /**
   * An A2UI surface, as `ThreadAgent` hands it to the runtime. On the wire it is
   * `a2ui-surface` ({@link A2UI_SURFACE}); see `thread-agent.ts` for why it is renamed.
   */
  surface: "vymalo.a2ui-surface",
} as const;

/** The ecosystem's activity type of an A2UI surface (ADR 0013), the one the orchestrator sends. */
export const A2UI_SURFACE = "a2ui-surface";

/** `metadata["vymalo.actor"]` of an attributed event: `{type, name, revision?}`. */
export const ACTOR_KEY = "vymalo.actor";

/** ADR 0008: the release travels in `forwardedProps` under the extension URI. */
export const RELEASE_CHANNELS_URI = "https://agents.vymalo.com/a2a/extensions/release-channels/v1";

/**
 * The data part name the runtime gives an `ACTIVITY_SNAPSHOT` of `activityType`:
 * `@assistant-ui/react-ag-ui` renders every activity but `a2ui-surface` and `mcp-apps` as the
 * data part `agui-activity/<activityType>`.
 */
export const activityPartName = (activityType: string) => `agui-activity/${activityType}`;

/**
 * The marker part `ThreadAgent` puts in front of an invocation's output (a `CUSTOM` event, which
 * the runtime turns into a data part of this name). It carries who ran and which revision, so a
 * text bubble, which the runtime reduces to bare text, can still show its actor.
 */
export const ACTOR_PART = "vymalo.actor";

/**
 * The contents of the activities, as the renderers read them. `actor` is not on the wire in the
 * content: the runtime drops an event's `metadata`, so `ThreadAgent` folds
 * `metadata["vymalo.actor"]` into the content it hands over.
 */
export type WithActor<T> = T & { actor?: ApiActor };
export type StatusContent = WithActor<{ status: AgentStatus; detail?: string }>;
export type ArtifactContent = WithActor<{
  name: string;
  mimeType?: string;
  uri?: string;
  text?: string;
}>;
export type ErrorContent = WithActor<{ message: string; retryable: boolean }>;
export type ActionContent = WithActor<{
  surfaceId: string;
  name: string;
  sourceComponentId?: string;
  context?: Record<string, unknown>;
}>;
/**
 * A surface: the operations exactly as the orchestrator sent them (untrusted, read by
 * `lib/a2ui/prepare.ts` and nothing else) and `surface`, the activity message id that identifies
 * the surface across updates. `actor` is the only label a surface gets.
 */
export type SurfaceContent = WithActor<{ a2ui_operations?: unknown; surface: string }>;

// ---- reading the contents -------------------------------------------------------------------
//
// A part's `data` is whatever the agent's activity carried: the renderers read it through these
// guards and show nothing for a shape they do not know (the UI renders events, never guesses).

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

const readActor = (v: unknown): ApiActor | undefined => {
  if (!isRecord(v)) return undefined;
  const name = str(v.name);
  const type = str(v.type);
  if (!name || (type !== "user" && type !== "agent" && type !== "system")) return undefined;
  const revision = str(v.revision);
  return { type, name, ...(revision ? { revision } : {}) };
};

export const parseActor = readActor;

export function parseStatus(v: unknown): StatusContent | null {
  if (!isRecord(v)) return null;
  const status = str(v.status);
  if (!status || !(AGENT_STATUSES as readonly string[]).includes(status)) return null;
  const detail = str(v.detail);
  const actor = readActor(v.actor);
  return {
    status: status as AgentStatus,
    ...(detail !== undefined ? { detail } : {}),
    ...(actor ? { actor } : {}),
  };
}

export function parseArtifact(v: unknown): ArtifactContent | null {
  if (!isRecord(v)) return null;
  const name = str(v.name);
  if (name === undefined) return null;
  const mimeType = str(v.mimeType);
  const uri = str(v.uri);
  const text = str(v.text);
  const actor = readActor(v.actor);
  return {
    name,
    ...(mimeType !== undefined ? { mimeType } : {}),
    ...(uri !== undefined ? { uri } : {}),
    ...(text !== undefined ? { text } : {}),
    ...(actor ? { actor } : {}),
  };
}

export function parseError(v: unknown): ErrorContent | null {
  if (!isRecord(v)) return null;
  const message = str(v.message);
  if (message === undefined) return null;
  const actor = readActor(v.actor);
  return { message, retryable: v.retryable === true, ...(actor ? { actor } : {}) };
}

export function parseAction(v: unknown): ActionContent | null {
  if (!isRecord(v)) return null;
  const surfaceId = str(v.surfaceId);
  const name = str(v.name);
  if (surfaceId === undefined || name === undefined) return null;
  const sourceComponentId = str(v.sourceComponentId);
  const actor = readActor(v.actor);
  return {
    surfaceId,
    name,
    ...(sourceComponentId !== undefined ? { sourceComponentId } : {}),
    ...(actor ? { actor } : {}),
  };
}

export function parseSurface(v: unknown): SurfaceContent | null {
  if (!isRecord(v)) return null;
  const surface = str(v.surface);
  if (surface === undefined) return null;
  const actor = readActor(v.actor);
  return {
    surface,
    ...("a2ui_operations" in v ? { a2ui_operations: v.a2ui_operations } : {}),
    ...(actor ? { actor } : {}),
  };
}
