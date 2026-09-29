import type { AgUiAssistantRuntime } from "@assistant-ui/react-ag-ui";

/**
 * A send the server refused left two things in the runtime: the user's message (added at once,
 * before the POST) and the empty reply of the run that never was. The transcript shows what the
 * server has, so both go, and the text is handed back for the composer.
 */
export function dropFailedSend(runtime: Pick<AgUiAssistantRuntime, "thread">): string | undefined {
  const messages = runtime.thread.getState().messages;
  const at = messages.findLastIndex((m) => m.role === "user");
  if (at === -1) return undefined;
  const failed = messages[at];
  const kept = messages.slice(0, at);
  runtime.thread.import({
    headId: kept.at(-1)?.id ?? null,
    messages: kept.map((message, i) => ({ parentId: kept[i - 1]?.id ?? null, message })),
  });
  return failed?.content.flatMap((p) => (p.type === "text" ? [p.text] : [])).join("\n");
}
