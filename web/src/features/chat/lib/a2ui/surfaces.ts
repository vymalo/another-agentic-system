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
