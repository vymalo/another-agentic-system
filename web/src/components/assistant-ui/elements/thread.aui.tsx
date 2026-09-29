"use client";

import {
  AuiIf,
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
import type { ApiActor } from "@/lib/api/types";
import { cn } from "@/lib/utils";

/*
 * Pruned from the assistant-ui `thread` registry item. The event log has no handlers for voice,
 * attachments, suggestions, feedback, reload, copy, edit or branches, so they are gone. What
 * stays is the viewport, the message layout and the scroll-to-bottom button. Data parts (status
 * lines, artifacts, errors) render through their registered UIs (features/chat/components/
 * data-uis.tsx) via `part.dataRendererUI`.
 */

const useActor = (): ApiActor | undefined =>
  useAuiState((s) => (s.message.metadata.custom as { actor?: ApiActor } | undefined)?.actor);
const useCreatedAt = (): Date | undefined => useAuiState((s) => s.message.createdAt);

// The thread has no messages yet and is done loading: nothing has happened so far.
const isEmptyView = (s: { thread: { messages: readonly unknown[]; isLoading: boolean } }) =>
  s.thread.messages.length === 0 && !s.thread.isLoading;
// The history is still being fetched.
const isLoadingView = (s: { thread: { messages: readonly unknown[]; isLoading: boolean } }) =>
  s.thread.messages.length === 0 && s.thread.isLoading;

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

/**
 * The transcript and, below it, the composer (`children`). The log is the `role="log"` live
 * region every test and screen reader relies on.
 */
export const Thread: FC<{ children?: ReactNode }> = ({ children }) => (
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
          <AuiIf condition={isLoadingView}>
            <ThreadHistorySkeleton />
          </AuiIf>
          <AuiIf condition={isEmptyView}>
            <p className="my-1 text-sm text-muted-foreground">Waiting for the first event…</p>
          </AuiIf>
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

/** Who posted a message, with the time of the event as a tooltip. */
const MessageActor: FC = () => {
  const actor = useActor();
  const createdAt = useCreatedAt();
  return <ActorLabel actor={actor} at={createdAt} />;
};

export const UserMessage: FC = () => (
  <MessagePrimitive.Root
    data-slot="user-message"
    data-role="user"
    className="flex min-w-0 flex-col items-end gap-1"
  >
    <div className="max-w-[min(100%,40rem)] rounded-lg bg-primary px-3.5 py-2 text-primary-foreground [overflow-wrap:anywhere] [&_a]:text-current">
      <MessagePrimitive.Parts components={{ Text: MarkdownText }} />
    </div>
    <MessageActor />
  </MessagePrimitive.Root>
);

const noGroups = groupPartByType({});

export const AssistantMessage: FC = () => {
  // A message whose first part is text is an agent's reply (a bubble); everything else is an
  // inline line or card that is not wrapped.
  const isText = useAuiState((s) => s.message.content[0]?.type === "text");
  return (
    <MessagePrimitive.Root
      data-slot="agent-message"
      data-role="assistant"
      className="flex min-w-0 flex-col items-start gap-1"
    >
      {isText ? <MessageActor /> : null}
      <div
        className={cn(
          isText
            ? "max-w-[min(100%,40rem)] rounded-lg border bg-muted px-3.5 py-2 [overflow-wrap:anywhere]"
            : "w-full",
        )}
      >
        <MessagePrimitive.GroupedParts groupBy={noGroups}>
          {({ part }) => {
            switch (part.type) {
              case "text":
                return <MarkdownText />;
              case "data":
                return part.dataRendererUI;
              default:
                return null;
            }
          }}
        </MessagePrimitive.GroupedParts>
      </div>
    </MessagePrimitive.Root>
  );
};
