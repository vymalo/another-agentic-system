"use client";

import {
  groupPartByType,
  MessagePrimitive,
  ThreadPrimitive,
  useAuiState,
} from "@assistant-ui/react";
import { ArrowDownIcon } from "lucide-react";
import type { FC, ReactNode } from "react";
import { MarkdownText } from "@/components/assistant-ui/elements/markdown-text";
import { TooltipIconButton } from "@/components/assistant-ui/elements/tooltip-icon-button";
import { Skeleton } from "@/components/ui/skeleton";
import { ActorLabel } from "@/features/chat/components/actor-label";
import { ACTOR_PART, parseActor } from "@/features/chat/lib/agui/vymalo";
import type { ApiActor } from "@/lib/api/types";

/*
 * Pruned from the assistant-ui `thread` registry item. The event log has no handlers for voice,
 * attachments, suggestions, feedback, reload, copy, edit or branches, so they are gone. What
 * stays is the viewport, the message layout and the scroll-to-bottom button.
 *
 * A run is one assistant message whose parts are, in order, the run's activities (status lines,
 * artifacts, errors: data parts, drawn by their registered UIs in features/chat/components/
 * data-uis.tsx via `part.dataRendererUI`) and the agent's text (bubbles). A `vymalo.actor` marker
 * part in front says who ran; the bubbles carry that label.
 */

const useCreatedAt = (): Date | undefined => useAuiState((s) => s.message.createdAt);

/** Who ran: the actor marker `ThreadAgent` puts in front of an invocation's parts. */
const useRunActor = (): ApiActor | undefined => {
  const data = useAuiState((s) => {
    const marker = s.message.content.find((p) => p.type === "data" && p.name === ACTOR_PART);
    return marker && marker.type === "data" ? marker.data : undefined;
  });
  return parseActor(data);
};

/** A user message injected from the connect stream carries its actor in the message metadata. */
const useUserActor = (): ApiActor | undefined => {
  // the selector returns the stored value itself: a fresh object per call would loop
  const actor = useAuiState(
    (s) => (s.message.metadata.custom as { actor?: unknown } | undefined)?.actor,
  );
  return parseActor(actor);
};

const ThreadHistorySkeleton: FC = () => (
  <div role="status" data-slot="aui_thread-history-skeleton" className="flex flex-col gap-3">
    <span className="sr-only">Loading conversation…</span>
    <Skeleton aria-hidden="true" className="ml-auto h-9 w-2/5 rounded-lg" />
    <Skeleton aria-hidden="true" className="h-5 w-3/5" />
    <Skeleton aria-hidden="true" className="h-5 w-2/5" />
  </div>
);

const ThreadScrollToBottom: FC = () => (
  <ThreadPrimitive.ScrollToBottom asChild>
    <TooltipIconButton
      tooltip="Scroll to bottom"
      aria-label="Scroll to bottom"
      variant="outline"
      className="absolute -top-12 z-10 size-8 self-center rounded-full p-0 disabled:invisible"
    >
      <ArrowDownIcon />
    </TooltipIconButton>
  </ThreadPrimitive.ScrollToBottom>
);

type ThreadProps = {
  /** The conversation is still being fetched: a skeleton while it is empty. */
  loading: boolean;
  /** Nothing has happened yet (loaded, and no event). */
  empty: boolean;
  children?: ReactNode;
};

/**
 * The transcript and, below it, the composer (`children`). The log is the `role="log"` live
 * region every test and screen reader relies on.
 */
export const Thread: FC<ThreadProps> = ({ loading, empty, children }) => {
  const noMessages = useAuiState((s) => s.thread.messages.length === 0);
  return (
    <ThreadPrimitive.Root className="@container flex min-h-0 flex-1 flex-col">
      <ThreadPrimitive.Viewport
        data-slot="aui_thread-viewport"
        className="relative flex flex-1 flex-col overflow-x-hidden overflow-y-auto scroll-smooth"
      >
        <div className="mx-auto flex w-full max-w-3xl flex-1 flex-col">
          <div
            role="log"
            aria-label="Conversation"
            data-slot="aui_message-group"
            className="flex flex-col gap-3 py-4"
          >
            {loading && noMessages ? <ThreadHistorySkeleton /> : null}
            {empty && noMessages ? (
              <p className="my-1 text-sm text-muted-foreground">Waiting for the first event…</p>
            ) : null}
            <ThreadPrimitive.Messages>
              {({ message }) => (message.role === "user" ? <UserMessage /> : <AssistantMessage />)}
            </ThreadPrimitive.Messages>
          </div>
          <ThreadPrimitive.ViewportFooter className="sticky bottom-0 mt-auto flex flex-col bg-background">
            <ThreadScrollToBottom />
            {children}
          </ThreadPrimitive.ViewportFooter>
        </div>
      </ThreadPrimitive.Viewport>
    </ThreadPrimitive.Root>
  );
};

export const UserMessage: FC = () => {
  const actor = useUserActor();
  const createdAt = useCreatedAt();
  return (
    <MessagePrimitive.Root
      data-slot="user-message"
      data-role="user"
      className="flex min-w-0 flex-col items-end gap-1"
    >
      <div className="max-w-[min(100%,40rem)] rounded-lg bg-primary px-3.5 py-2 text-primary-foreground [overflow-wrap:anywhere] [&_a]:text-current">
        <MessagePrimitive.Parts components={{ Text: MarkdownText }} />
      </div>
      <ActorLabel actor={actor} at={createdAt} />
    </MessagePrimitive.Root>
  );
};

const noGroups = groupPartByType({});

/** The agent's words: a bubble, with who said it above. */
const AgentText: FC = () => {
  const actor = useRunActor();
  const createdAt = useCreatedAt();
  return (
    <div
      data-slot="agent-message"
      data-role="assistant"
      className="flex min-w-0 max-w-[min(100%,40rem)] flex-col items-start gap-1"
    >
      <ActorLabel actor={actor} at={createdAt} />
      <div className="rounded-lg border bg-muted px-3.5 py-2 [overflow-wrap:anywhere]">
        <MarkdownText />
      </div>
    </div>
  );
};

export const AssistantMessage: FC = () => (
  <MessagePrimitive.Root
    data-slot="agent-run"
    className="flex min-w-0 flex-col items-start gap-3 empty:hidden"
  >
    <MessagePrimitive.GroupedParts groupBy={noGroups}>
      {({ part }) => {
        switch (part.type) {
          case "text":
            return <AgentText />;
          case "data":
            return <div className="w-full empty:hidden">{part.dataRendererUI}</div>;
          default:
            return null;
        }
      }}
    </MessagePrimitive.GroupedParts>
  </MessagePrimitive.Root>
);
