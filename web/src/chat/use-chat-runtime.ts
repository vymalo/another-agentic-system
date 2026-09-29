import {
  type AppendMessage,
  type AssistantRuntime,
  useExternalStoreRuntime,
} from "@assistant-ui/react";
import { useRouter } from "next/navigation";
import { useMemo, useRef } from "react";
import { api, problemMessage } from "@/api/client";
import { isActive, isTerminal } from "@/api/types";
import { visibleEvents } from "./event-log";
import { type ChatItem, convertMessage, toItems } from "./to-items";
import type { ThreadView } from "./use-thread";
import type { ThreadsView } from "./use-threads";

export type Selection = { agentId: string | null; release: string | null };

type Args = {
  threadId: string | null;
  view: ThreadView;
  threads: ThreadsView;
  selection: Selection;
  onSendError: (message: string | null) => void;
};

const textOf = (message: AppendMessage) =>
  message.content
    .flatMap((p) => (p.type === "text" ? [p.text] : []))
    .join("\n")
    .trim();

/**
 * assistant-ui external-store runtime over the event log.
 *
 * Messages are events (never optimistic guesses): a sent message shows up when the server
 * returns it (202 body or the SSE echo). Running state is the server's thread state.
 */
export function useChatRuntime({
  threadId,
  view,
  threads,
  selection,
  onSendError,
}: Args): AssistantRuntime {
  const router = useRouter();
  const runtimeRef = useRef<AssistantRuntime | null>(null);

  const items = useMemo(() => toItems(visibleEvents(view.log)), [view.log]);
  const { state, addEvents, loaded } = view;

  const runtime = useExternalStoreRuntime<ChatItem>({
    messages: items,
    convertMessage,
    isRunning: isActive(state),
    isDisabled: threadId !== null && isTerminal(state),
    isSendDisabled: threadId === null && !selection.agentId,
    isLoading: !loaded,
    onNew: async (message) => {
      const text = textOf(message);
      if (!text) return;
      onSendError(null);
      const restore = (reason: string) => {
        onSendError(reason);
        runtimeRef.current?.thread.composer.setText(text);
      };
      try {
        if (threadId === null) {
          if (!selection.agentId) return restore("Choose an agent first.");
          const { data, error } = await api.POST("/api/threads", {
            body: {
              target: {
                agentId: selection.agentId,
                ...(selection.release ? { release: selection.release } : {}),
              },
              text,
            },
          });
          if (!data) return restore(problemMessage(error));
          router.push(`/threads/${data.id}`);
          return;
        }
        const { data, error, response } = await api.POST("/api/threads/{threadId}/messages", {
          params: { path: { threadId } },
          body: { text },
        });
        if (!data) {
          return restore(
            response.status === 409
              ? "This thread is finished. Start a new thread to continue."
              : problemMessage(error),
          );
        }
        addEvents([data]); // a real server event with a seq; the SSE echo is deduped
      } catch (e) {
        restore(problemMessage(e));
      }
    },
    onCancel: async () => {
      if (threadId === null) return;
      const { error } = await api.POST("/api/threads/{threadId}/cancel", {
        params: { path: { threadId } },
      });
      if (error) onSendError(problemMessage(error));
    },
    adapters: {
      threadList: {
        ...(threadId !== null ? { threadId } : {}),
        isLoading: threads.loading,
        threads: threads.threads.map((t) => ({
          status: "regular" as const,
          id: t.id,
          title: t.title,
        })),
        onSwitchToNewThread: () => router.push("/"),
        onSwitchToThread: (id) => router.push(`/threads/${id}`),
      },
    },
  });
  runtimeRef.current = runtime;
  return runtime;
}
