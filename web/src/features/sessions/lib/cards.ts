/**
 * The cards a child session is told by, read from what the stream carries
 * (docs/api/agui.md "Child sessions as subagents" and "A person starts, sends and closes a session"):
 *
 * - `vymalo.session` in the **parent**, one card for each session (`session-<seq>`), said again at
 *   every event of it;
 * - `vymalo.session` in the **child** (`session-self`), the child's own state, which has no
 *   `threadId` and no description;
 * - `vymalo.sessionStart` in the parent, the request of a person's start (`session-start-<request>`);
 * - `Thread.childOf`, which says that a thread is a helper's.
 *
 * Every reader is strict and total: the enums are closed (a word it does not know is a bad shape), a
 * member it does not know is ignored, a bad shape is `null`, and a `threadId` that is not a UUID
 * gives no link (a card never makes a route out of text). The agent's words (`agent`, `description`,
 * `reason`) are untrusted and drawn as text, never as markup.
 */

/** The `activityType` of the session card, in the parent and in the child. */
export const ACTIVITY_SESSION = "vymalo.session";
/** The `activityType` of a person's start. */
export const ACTIVITY_SESSION_START = "vymalo.sessionStart";
/** The `messageId` of the child's own card. */
export const SESSION_SELF_ID = "session-self";
/** The metadata of a session's `SUBAGENT_STARTED` that names the child thread. */
export const CHILD_THREAD_KEY = "vymalo.childThread";

export const SESSION_STATES = ["working", "idle", "closed"] as const;
export type SessionState = (typeof SESSION_STATES)[number];
export const SESSION_REPORTS = ["done", "failed", "canceled", "blocked"] as const;
export type SessionReport = (typeof SESSION_REPORTS)[number];
export const START_STATES = ["starting", "opened", "failed"] as const;
export type StartState = (typeof START_STATES)[number];
export const SESSION_BYS = ["agent", "person"] as const;
export type SessionBy = (typeof SESSION_BYS)[number];

/** A session as the parent's card, or the child's own card, says it. */
export type SessionCard = {
  /** The agent's own id of the session: opaque. */
  session: string;
  /** The child thread, a UUID; absent on the child's own card and when the card's is not one. */
  threadId?: string;
  /** The helper's name (the agent's text). */
  agent: string;
  /** What it was started for (the agent's text); empty on the child's own card. */
  description: string;
  /** Whether the session goes idle after a round instead of closing. */
  keepOpen: boolean;
  state: SessionState;
  /** Only `person` is ever said: an agent's own sessions leave it out. */
  by?: SessionBy;
  /** The newest round the log names, and how it ended once it did. */
  round?: number;
  report?: SessionReport;
  /** A close waits for the round to end, or is on its way (the child's own card). */
  closing: boolean;
  /** A person asked for the close and it is not answered yet (the child's own card). */
  closeAsked: boolean;
  /** When the event happened, as the card says it. */
  at?: string;
};

/** The card is at least as new as the resource read at `updatedAt`; a resource with no usable time is the older. */
export function cardNewer(card: Pick<SessionCard, "at">, updatedAt: string | undefined): boolean {
  const at = Date.parse(card.at ?? "");
  if (Number.isNaN(at)) return false;
  const read = Date.parse(updatedAt ?? "");
  return Number.isNaN(read) || at >= read;
}

/** A start a person asked for, as the parent's card says it. */
export type SessionStartCard = {
  request: string;
  agent: string;
  description: string;
  keepOpen: boolean;
  state: StartState;
  /** With `opened`: the child thread (a UUID) and the session. */
  threadId?: string;
  session?: string;
  /** With `failed`: the agent's own words (untrusted). */
  reason?: string;
  at?: string;
};

/** What a thread says of being a helper's (`Thread.childOf`). */
export type ChildOf = {
  /** The parent thread, a UUID; absent when the thread's is not one. */
  parent?: string;
  session: string;
  agent?: string;
  state?: SessionState;
  keepOpen: boolean;
  closing: boolean;
  closeAsked: boolean;
};

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** A thread id the card can make a link of: a UUID, else nothing. */
const uuidOrNone = (value: unknown): string | undefined =>
  typeof value === "string" && UUID.test(value) ? value : undefined;

const oneOf = <T extends string>(list: readonly T[], value: unknown): T | undefined =>
  typeof value === "string" ? list.find((x) => x === value) : undefined;

/** A member that is absent, or that is the type: `false` for a member that is there and is not. */
const optional = (value: unknown, is: (v: unknown) => boolean): boolean =>
  value === undefined || is(value);
const isString = (v: unknown): v is string => typeof v === "string";
const isBoolean = (v: unknown): v is boolean => typeof v === "boolean";
const isRound = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v) && v >= 1;
const nonEmpty = (v: unknown): v is string => typeof v === "string" && v !== "";

/** The parent's `vymalo.session` card, or the child's `session-self`; `null` for a bad shape. */
export function parseSession(content: unknown): SessionCard | null {
  if (!isRecord(content)) return null;
  const { session, agent, description, keepOpen, round, closing, closeAsked, at } = content;
  const state = oneOf(SESSION_STATES, content.state);
  if (!nonEmpty(session) || !isString(agent) || !isBoolean(keepOpen) || state === undefined) {
    return null;
  }
  if (!optional(description, isString)) return null;
  if (!optional(round, isRound)) return null;
  if (!optional(closing, isBoolean) || !optional(closeAsked, isBoolean)) return null;
  if (!optional(at, isString)) return null;
  const by = content.by === undefined ? undefined : oneOf(SESSION_BYS, content.by);
  if (content.by !== undefined && by === undefined) return null;
  const report = content.report === undefined ? undefined : oneOf(SESSION_REPORTS, content.report);
  if (content.report !== undefined && report === undefined) return null;
  const threadId = uuidOrNone(content.threadId);
  return {
    session,
    ...(threadId ? { threadId } : {}),
    agent,
    description: typeof description === "string" ? description : "",
    keepOpen,
    state,
    ...(by ? { by } : {}),
    ...(typeof round === "number" ? { round } : {}),
    ...(report ? { report } : {}),
    closing: closing === true,
    closeAsked: closeAsked === true,
    ...(typeof at === "string" ? { at } : {}),
  };
}

/** The parent's `vymalo.sessionStart` card; `null` for a bad shape. */
export function parseSessionStart(content: unknown): SessionStartCard | null {
  if (!isRecord(content)) return null;
  const { request, agent, description, keepOpen, session, reason, at } = content;
  const state = oneOf(START_STATES, content.state);
  if (
    !nonEmpty(request) ||
    !isString(agent) ||
    !isString(description) ||
    !isBoolean(keepOpen) ||
    state === undefined
  ) {
    return null;
  }
  if (!optional(session, isString) || !optional(reason, isString) || !optional(at, isString)) {
    return null;
  }
  const threadId = uuidOrNone(content.threadId);
  return {
    request,
    agent,
    description,
    keepOpen,
    state,
    ...(threadId ? { threadId } : {}),
    ...(typeof session === "string" ? { session } : {}),
    ...(typeof reason === "string" ? { reason } : {}),
    ...(typeof at === "string" ? { at } : {}),
  };
}

/** `Thread.childOf` of a thread: the helper it is a session of, or `null` when it is not one. */
export function childOf(
  thread: { childOf?: unknown; [member: string]: unknown } | null | undefined,
): ChildOf | null {
  const row = thread?.childOf;
  if (!isRecord(row)) return null;
  const { session, agent, keepOpen, closing, closeAsked } = row;
  if (!nonEmpty(session)) return null;
  if (!optional(agent, isString)) return null;
  if (!optional(keepOpen, isBoolean) || !optional(closing, isBoolean)) return null;
  if (!optional(closeAsked, isBoolean)) return null;
  const state = row.state === undefined ? undefined : oneOf(SESSION_STATES, row.state);
  if (row.state !== undefined && state === undefined) return null;
  const parent = uuidOrNone(row.threadId);
  return {
    ...(parent ? { parent } : {}),
    session,
    ...(typeof agent === "string" ? { agent } : {}),
    ...(state ? { state } : {}),
    keepOpen: keepOpen === true,
    closing: closing === true,
    closeAsked: closeAsked === true,
  };
}
