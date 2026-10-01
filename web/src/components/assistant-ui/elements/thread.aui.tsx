"use client";

import {
  MessagePrimitive,
  type PartState,
  ThreadPrimitive,
  useAuiState,
} from "@assistant-ui/react";
import { ArrowDownIcon, MessageCircleQuestionIcon } from "lucide-react";
import type { FC, ReactNode } from "react";
import { MarkdownText } from "@/components/assistant-ui/elements/markdown-text";
import { TooltipIconButton } from "@/components/assistant-ui/elements/tooltip-icon-button";
import { BrandMark } from "@/components/brand-mark";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { AnswerBubble } from "@/features/chat/components/answer-bubble";
import { TurnCards } from "@/features/chat/components/cards/turn-cards";
import { StepList } from "@/features/chat/components/steps/step-list";
import { useThreadView } from "@/features/chat/components/thread-view";
import {
  ACTIVITY,
  ACTOR_PART,
  activityPartName,
  parseActor,
  parseAnswers,
  parseArtifact,
  parseStatus,
} from "@/features/chat/lib/agui/vymalo";
import { drawsStep, isAnswerPart, isCardArtifact, isStepPart } from "@/features/chat/lib/steps";
import type { ApiActor } from "@/lib/api/types";
import { isActive } from "@/lib/api/types";

/*
 * Pruned from the assistant-ui `thread` registry item and rebuilt as a classical chat
 * (web/DESIGN.md). The person's words are a soft bubble on the right. A run is one assistant
 * message, drawn as a turn: the agent's mark and name once, then its parts in order, where every
 * stretch of activities (statuses, artifacts, checks, CI reports, reworks, actions) is one compact
 * step list, the agent's words are prose, and a failure or a surface stands on its own; the pull
 * requests and files it shared follow as cards. A `vymalo.actor` marker part says who ran.
 */

type AnyPart = { type: string; name?: string; data?: unknown; text?: string };

const useCreatedAt = (): Date | undefined => useAuiState((s) => s.message.createdAt);

/** Who ran: the actor marker `ThreadAgent` puts in front of an invocation's parts. */
const useRunActor = (): ApiActor | undefined => {
  const data = useAuiState((s) => {
    const marker = s.message.content.find((p) => p.type === "data" && p.name === ACTOR_PART);
    return marker && marker.type === "data" ? marker.data : undefined;
  });
  return parseActor(data);
};

/** Whether a part draws anything: words, a step, a callout, a surface or a card. */
function drawsSomething(part: AnyPart): boolean {
  if (part.type === "text") return Boolean(part.text?.trim());
  if (part.type !== "data" || !part.name) return false;
  if (isStepPart(part)) {
    if (drawsStep(part)) return true;
    const artifact =
      part.name === activityPartName(ACTIVITY.artifact) ? parseArtifact(part.data) : null;
    return artifact !== null && isCardArtifact(artifact);
  }
  if (part.name === activityPartName(ACTIVITY.status)) {
    return parseStatus(part.data)?.status === "failed";
  }
  return (
    part.name === activityPartName(ACTIVITY.error) ||
    part.name === activityPartName(ACTIVITY.surface)
  );
}

/** Every stretch of step parts is one group, drawn as one list. */
const byStep = (part: PartState): readonly "group-steps"[] =>
  isStepPart(part as AnyPart) ? ["group-steps"] : [];

const ThreadHistorySkeleton: FC = () => (
  <div role="status" data-slot="aui_thread-history-skeleton" className="flex flex-col gap-4">
    <span className="sr-only">Loading conversation…</span>
    <Skeleton aria-hidden="true" className="ml-auto h-10 w-2/5 rounded-[20px]" />
    <div className="flex items-center gap-2.5">
      <Skeleton aria-hidden="true" className="size-7 rounded-full" />
      <Skeleton aria-hidden="true" className="h-4 w-24" />
    </div>
    <Skeleton aria-hidden="true" className="h-4 w-3/5 sm:ml-10" />
    <Skeleton aria-hidden="true" className="h-4 w-2/5 sm:ml-10" />
  </div>
);

const ThreadScrollToBottom: FC = () => (
  <ThreadPrimitive.ScrollToBottom asChild>
    <TooltipIconButton
      tooltip="Scroll to bottom"
      aria-label="Scroll to bottom"
      variant="outline"
      className="absolute -top-11 z-10 size-9 self-center rounded-full bg-background p-0 shadow-composer disabled:invisible"
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
 * The transcript in a centered reading column and, below it, the composer (`children`), sticky at
 * the bottom. The log is the `role="log"` live region every test and screen reader relies on.
 */
export const Thread: FC<ThreadProps> = ({ loading, empty, children }) => {
  const noMessages = useAuiState((s) => s.thread.messages.length === 0);
  return (
    <ThreadPrimitive.Root className="@container flex min-h-0 flex-1 flex-col">
      <ThreadPrimitive.Viewport
        data-slot="aui_thread-viewport"
        className="relative flex flex-1 flex-col overflow-x-hidden overflow-y-auto scroll-smooth"
      >
        <div className="mx-auto flex w-full max-w-3xl flex-1 flex-col px-4 md:px-6">
          <div
            role="log"
            aria-label="Conversation"
            data-slot="aui_message-group"
            className="flex flex-col gap-8 pt-4 pb-10 md:pt-8"
          >
            {loading && noMessages ? <ThreadHistorySkeleton /> : null}
            {empty && noMessages ? (
              <p className="text-sm text-muted-foreground">Waiting for the first event…</p>
            ) : null}
            <ThreadPrimitive.Messages>
              {({ message }) => (message.role === "user" ? <UserMessage /> : <AssistantMessage />)}
            </ThreadPrimitive.Messages>
            <StartingTurn />
          </div>
          <ThreadPrimitive.ViewportFooter className="sticky bottom-0 mt-auto flex flex-col bg-[linear-gradient(to_top,var(--background)_75%,transparent)] pt-3">
            <ThreadScrollToBottom />
            {children}
          </ThreadPrimitive.ViewportFooter>
        </div>
      </ThreadPrimitive.Viewport>
    </ThreadPrimitive.Root>
  );
};

/**
 * The time something happened: a tooltip on hover for the eye, and the same time as visually
 * hidden text after the label, so a screen reader and the keyboard get it too (it is secondary,
 * so it is no tab stop of its own).
 */
function WithTime({ at, children }: { at: Date | undefined; children: ReactNode }) {
  if (!at || Number.isNaN(at.getTime())) return <>{children}</>;
  const time = <time dateTime={at.toISOString()}>{at.toLocaleString()}</time>;
  return (
    <>
      <Tooltip>
        <TooltipTrigger asChild>{children}</TooltipTrigger>
        <TooltipContent side="bottom">{time}</TooltipContent>
      </Tooltip>
      <span data-slot="message-time" className="sr-only">
        {time}
      </span>
    </>
  );
}

export const UserMessage: FC = () => {
  const createdAt = useCreatedAt();
  return (
    <MessagePrimitive.Root
      data-slot="user-message"
      data-role="user"
      className="flex min-w-0 motion-safe:animate-turn-in flex-col items-end"
    >
      <WithTime at={createdAt}>
        <div className="max-w-[85%] rounded-[20px] rounded-tr-md bg-bubble px-4 py-2.5 text-[0.9375rem] leading-6 [&_.aui-md-p]:leading-6 [overflow-wrap:anywhere] sm:max-w-[80%] [&_.aui-md-inline-code]:bg-background/70">
          <MessagePrimitive.Parts components={{ Text: MarkdownText }} />
        </div>
      </WithTime>
    </MessagePrimitive.Root>
  );
};

/** The agent's mark and name, once per turn: `coder · coder-r47` (the name, then the revision). */
function TurnHeader({ actor, at }: { actor: ApiActor | undefined; at?: Date | undefined }) {
  const { agentId } = useThreadView();
  const name = actor?.name ?? agentId;
  return (
    <div className="flex min-w-0 items-center gap-2.5">
      <BrandMark />
      {name ? (
        <WithTime at={at}>
          <span data-slot="actor-label" className="min-w-0 truncate text-sm">
            <span className="font-medium text-foreground capitalize">{name}</span>
            {actor?.revision ? (
              <span className="text-xs text-muted-foreground"> · {actor.revision}</span>
            ) : null}
          </span>
        </WithTime>
      ) : null}
    </div>
  );
}

/** The agent's words: prose, no bubble. The last words of a waiting turn are the question. */
const AgentText: FC<{ question: boolean }> = ({ question }) => (
  <div data-slot="agent-message" data-role="assistant" className="min-w-0 [overflow-wrap:anywhere]">
    <div className="text-[0.9375rem] leading-7 text-foreground">
      <MarkdownText />
    </div>
    {question ? (
      <p
        data-slot="your-turn"
        className="mt-3 inline-flex items-center gap-1.5 rounded-full bg-warning-soft px-2.5 py-1 text-xs font-medium text-warning"
      >
        <MessageCircleQuestionIcon aria-hidden="true" className="size-3.5" />
        Waiting for your reply
      </p>
    ) : null}
  </div>
);

/** "Coder is starting…": the shimmering line of a turn that has nothing to show yet. */
function Starting({ name }: { name: string | null }) {
  return (
    <p data-slot="starting" className="text-shimmer text-sm font-medium">
      {name ? <span className="capitalize">{name}</span> : "The agent"} is starting…
    </p>
  );
}

/** A run that has not produced its first event: the agent's header and the starting line. */
function StartingTurn() {
  const { state, agentId } = useThreadView();
  const lastIsUser = useAuiState((s) => s.thread.messages.at(-1)?.role === "user");
  if (!lastIsUser || !isActive(state)) return null;
  return (
    <div data-slot="agent-turn" className="flex motion-safe:animate-turn-in flex-col gap-3">
      <TurnHeader actor={undefined} />
      <div className="sm:pl-10">
        <Starting name={agentId} />
      </div>
    </div>
  );
}

export const AssistantMessage: FC = () => {
  const actor = useRunActor();
  const createdAt = useCreatedAt();
  const { waiting, agentId } = useThreadView();
  const content = useAuiState((s) => s.message.content) as readonly AnyPart[];
  const running = useAuiState((s) => s.message.status?.type === "running");
  const isLast = useAuiState((s) => s.message.isLast);

  let lastDrawn = -1;
  let lastText = -1;
  content.forEach((p, i) => {
    if (drawsSomething(p)) lastDrawn = i;
    if (p.type === "text" && p.text?.trim()) lastText = i;
  });
  // what the person answered through a Choices: their words, so above the agent's mark, not a step
  const answers = content.flatMap((p, i) => {
    const data = isAnswerPart(p) ? parseAnswers(p.data) : null;
    return data ? [{ i, data }] : [];
  });
  if (lastDrawn < 0 && !running && answers.length === 0) return null;
  const lastTextValue = lastText >= 0 ? content[lastText]?.text : undefined;
  const lastIsSteps = lastDrawn >= 0 && isStepPart(content[lastDrawn] as AnyPart);
  const answered = (
    <>
      {answers.map(({ i, data }) => (
        <AnswerBubble key={i} data={data} />
      ))}
    </>
  );
  if (lastDrawn < 0 && !running) {
    return (
      <MessagePrimitive.Root data-slot="answer-turn" className="flex min-w-0 flex-col gap-3">
        {answered}
      </MessagePrimitive.Root>
    );
  }

  return (
    <MessagePrimitive.Root
      data-slot="agent-turn"
      className="flex min-w-0 motion-safe:animate-turn-in flex-col gap-3"
    >
      {answers.length > 0 ? <div className="mb-3 flex flex-col gap-3">{answered}</div> : null}
      <TurnHeader actor={actor} at={createdAt} />
      <div className="flex min-w-0 flex-col gap-4 sm:pl-10">
        <MessagePrimitive.GroupedParts groupBy={byStep} indicator="always">
          {({ part }) => {
            switch (part.type) {
              case "group-steps":
                return (
                  <StepList
                    indices={part.indices}
                    last={part.indices.includes(lastDrawn) || lastDrawn < 0}
                  />
                );
              case "text":
                return (
                  <AgentText
                    question={waiting && isLast && lastText >= 0 && part.text === lastTextValue}
                  />
                );
              case "data":
                // the person's answers are drawn above the turn
                if (isAnswerPart(part as AnyPart)) return null;
                return <div className="w-full empty:hidden">{part.dataRendererUI}</div>;
              case "indicator":
                if (lastDrawn < 0) return <Starting name={actor?.name ?? agentId} />;
                return lastIsSteps ? null : (
                  <p data-slot="working" className="text-shimmer text-sm">
                    Working…
                  </p>
                );
              default:
                return null;
            }
          }}
        </MessagePrimitive.GroupedParts>
        <TurnCards />
      </div>
    </MessagePrimitive.Root>
  );
};
