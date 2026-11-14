import { ACTIVITY, activityPartName } from "@/features/chat/lib/agui/vymalo";
import { isPlanSurface } from "./limits";

type Part = { type: string; name?: string; data?: unknown };
type Message = { id: string; role: string; content: readonly Part[] };

const isRecord = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * The newest copy of a surface in the transcript. A surface is one activity message id
 * (`a2ui-<seq>`); an update inside a run replaces the part in place, but an update in a LATER run
 * is a part of that run's message, and the earlier message still holds the old copy. Only the
 * newest copy is live (its actions can be used).
 */
export function latestSurface(
  messages: readonly Message[],
  key: string,
): { messageId: string; data: unknown } | undefined {
  const name = activityPartName(ACTIVITY.surface);
  for (let i = messages.length - 1; i >= 0; i--) {
    const message = messages[i];
    if (message?.role !== "assistant") continue;
    for (let j = message.content.length - 1; j >= 0; j--) {
      const part = message.content[j];
      if (
        part?.type === "data" &&
        part.name === name &&
        isRecord(part.data) &&
        part.data.surface === key
      ) {
        return { messageId: message.id, data: part.data };
      }
    }
  }
  return undefined;
}

const OPERATION_KEYS = ["createSurface", "updateComponents", "updateDataModel", "deleteSurface"];

/** The A2UI surface id the operations of a surface activity name (the first operation that names one). */
export function surfaceIdOf(operations: unknown): string | undefined {
  if (!Array.isArray(operations)) return undefined;
  for (const op of operations) {
    if (!isRecord(op)) continue;
    for (const k of OPERATION_KEYS) {
      const body = op[k];
      if (isRecord(body) && typeof body.surfaceId === "string") return body.surfaceId;
    }
  }
  return undefined;
}

/** Whether the newest operation of a copy deletes its surface (the copy says the plan is gone). */
function isDeletedCopy(operations: unknown): boolean {
  if (!Array.isArray(operations)) return false;
  const last = operations.at(-1);
  return isRecord(last) && isRecord(last.deleteSurface);
}

type Copy = { messageId: string; key: string; data: unknown; stretch: number };

/**
 * Every copy of the plan `surfaceId` in the transcript, in order, with the stretch it is in. The copies are
 * matched by the A2UI `surfaceId` and by nothing else: a run that starts again gives a copy a new activity key
 * (`a2ui-<seq>`), and it is the same plan. The thread is cut into stretches by the messages of a person
 * (`role: "user"`; a steer is one): docs/api/ui-catalog-v1.md, "A surface sent again", rule 4.
 */
function copiesOfPlan(messages: readonly Message[], surfaceId: string): Copy[] {
  const name = activityPartName(ACTIVITY.surface);
  const out: Copy[] = [];
  let stretch = 0;
  for (const message of messages) {
    if (message.role === "user") stretch += 1;
    if (message.role !== "assistant") continue;
    for (const part of message.content) {
      if (part.type !== "data" || part.name !== name || !isRecord(part.data)) continue;
      if (surfaceIdOf(part.data.a2ui_operations) !== surfaceId) continue;
      out.push({
        messageId: message.id,
        key: String(part.data.surface),
        data: part.data,
        stretch,
      });
    }
  }
  return out;
}

/**
 * What the copy of a plan in message `messageId` (activity `key`) does: `live` it stands first in the
 * thread's newest drawing and shows the newest copy of that stretch; `frozen` it stands first in an earlier
 * drawing and shows the newest copy of ITS stretch (what the person saw when they wrote), under a line; `nothing`
 * it is a later copy of a drawing that is drawn where its first copy stands. The data of the copy to show
 * is {@link planDataOf}.
 */
export function planPlaceOf(
  messages: readonly Message[],
  messageId: string,
  key: string,
  surfaceId: string,
): "live" | "frozen" | "nothing" {
  const copies = copiesOfPlan(messages, surfaceId);
  const me = copies.find((c) => c.messageId === messageId && c.key === key);
  if (!me) return "nothing";
  const first = copies.find((c) => c.stretch === me.stretch);
  if (first !== me) return "nothing";
  return me.stretch === copies.at(-1)?.stretch ? "live" : "frozen";
}

/** The data of the copy that the drawing standing at (`messageId`, `key`) shows: the newest of its stretch. */
export function planDataOf(
  messages: readonly Message[],
  messageId: string,
  key: string,
  surfaceId: string,
): unknown {
  const copies = copiesOfPlan(messages, surfaceId);
  const me = copies.find((c) => c.messageId === messageId && c.key === key);
  return me ? copies.filter((c) => c.stretch === me.stretch).at(-1)?.data : undefined;
}

/**
 * What the copy of a surface that sits in message `messageId` draws (docs/api/ui-catalog-v1.md, "A
 * surface sent again", rules 3 and 4): `own` its own operations, as the newest copy of a surface that is
 * not a plan does; `older` one line saying the surface was updated further down; for a plan `newest`
 * (the drawing is the live one), `frozen` (an earlier drawing) or `nothing`. A primitive on purpose: it
 * is read by a selector of the transcript.
 */
export function placeOf(
  messages: readonly Message[],
  messageId: string,
  key: string,
  surfaceId: string,
): "own" | "older" | "newest" | "frozen" | "nothing" {
  if (isPlanSurface(surfaceId)) {
    const place = planPlaceOf(messages, messageId, key, surfaceId);
    return place === "live" ? "newest" : place;
  }
  const newest = latestSurface(messages, key);
  return newest === undefined || newest.messageId === messageId ? "own" : "older";
}

/**
 * The plan the thread pins: the plan sent last that still exists (its newest copy does not delete it), as its
 * newest copy. A thread may hold several plans (`plan-a`, then `plan-b`); a plan that was deleted is not
 * pinned and the one before it is. None, no tab.
 */
export function latestPlan(
  messages: readonly Message[],
): { key: string; surfaceId: string; data: unknown } | undefined {
  const name = activityPartName(ACTIVITY.surface);
  const seen = new Set<string>();
  for (let i = messages.length - 1; i >= 0; i--) {
    const message = messages[i];
    if (message?.role !== "assistant") continue;
    for (let j = message.content.length - 1; j >= 0; j--) {
      const part = message.content[j];
      if (part?.type !== "data" || part.name !== name || !isRecord(part.data)) continue;
      const surfaceId = surfaceIdOf(part.data.a2ui_operations);
      if (surfaceId === undefined || !isPlanSurface(surfaceId) || seen.has(surfaceId)) continue;
      seen.add(surfaceId);
      if (isDeletedCopy(part.data.a2ui_operations)) continue;
      if (typeof part.data.surface === "string") {
        return { key: part.data.surface, surfaceId, data: part.data };
      }
    }
  }
  return undefined;
}

/** Whether these operations are about A2UI surface `surfaceId` (not the activity's message id). */
function namesSurface(operations: unknown, surfaceId: string): boolean {
  if (!Array.isArray(operations)) return false;
  return operations.some(
    (op) =>
      isRecord(op) &&
      OPERATION_KEYS.some(
        (k) => isRecord(op[k]) && (op[k] as Record<string, unknown>).surfaceId === surfaceId,
      ),
  );
}

/**
 * The operations of the copy of A2UI surface `surfaceId` that a person answered in message
 * `messageId` (the run of their action): the newest copy in the messages before it, else one in
 * the message itself. The operations are the agent's, untrusted, and the same array the renderer
 * read, so the same reference while the transcript holds it. `undefined` when the transcript has
 * no such surface (the answer is then shown with the raw ids and values).
 */
export function surfaceOperations(
  messages: readonly Message[],
  messageId: string,
  surfaceId: string,
): unknown {
  const name = activityPartName(ACTIVITY.surface);
  const at = messages.findIndex((m) => m.id === messageId);
  const order =
    at < 0 ? messages.map((_, i) => i).reverse() : [...Array(at).keys()].reverse().concat(at);
  for (const i of order) {
    const message = messages[i];
    if (message?.role !== "assistant") continue;
    for (let j = message.content.length - 1; j >= 0; j--) {
      const part = message.content[j];
      if (part?.type !== "data" || part.name !== name || !isRecord(part.data)) continue;
      if (namesSurface(part.data.a2ui_operations, surfaceId)) return part.data.a2ui_operations;
    }
  }
  return undefined;
}

/**
 * Whether the person has already acted on the thread since message `messageId` drew a form: a
 * `vymalo.action` (on this surface or a later one) or a plain message of theirs comes after it in the
 * transcript. A form that was sent stays answered when the thread waits again, say because the agent
 * asked again after refusing the answer (docs/api/ui-catalog-v1.md, "Answered forms"): the form asked
 * last is the open one, and a message sent while a form is open answers the ask and closes it.
 */
export function answeredAfter(messages: readonly Message[], messageId: string): boolean {
  const at = messages.findIndex((m) => m.id === messageId);
  if (at < 0) return false;
  const name = activityPartName(ACTIVITY.action);
  return messages
    .slice(at + 1)
    .some((m) => m.role === "user" || m.content.some((p) => p.type === "data" && p.name === name));
}
