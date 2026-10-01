import { ACTIVITY, activityPartName } from "@/features/chat/lib/agui/vymalo";

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
