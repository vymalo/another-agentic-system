"use client";

import {
  MessagePrimitive,
  type PartState,
  ThreadPrimitive,
  useAui,
  useAuiState,
} from "@assistant-ui/react";
import { ArrowDownIcon, MessageCircleQuestionIcon, PencilIcon } from "lucide-react";
import { type FC, type ReactNode, useRef, useState } from "react";
import { MarkdownText } from "@/components/assistant-ui/elements/markdown-text";
import { MessageEditor } from "@/components/assistant-ui/elements/message-editor";
import { TooltipIconButton } from "@/components/assistant-ui/elements/tooltip-icon-button";
import { TurnActions } from "@/components/assistant-ui/elements/turn-actions";
import { AgentAvatar } from "@/components/brand/agent-avatar";
import { Skeleton } from "@/components/ui/skeleton";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { AnswerBubble } from "@/features/chat/components/answer-bubble";
import { TurnCards } from "@/features/chat/components/cards/turn-cards";
import { DeliveryNote } from "@/features/chat/components/delivery-note";
import { LiveDraft, useLiveDrafts } from "@/features/chat/components/live-drafts";
import { TurnSummaryLine } from "@/features/chat/components/steps/turn-summary";
import { useThreadView } from "@/features/chat/components/thread-view";
import { drawnDrafts } from "@/features/chat/lib/agui/live-drafts";
import {
  ACTIVITY,
  ACTOR_PART,
  activityPartName,
  parseActor,
  parseActorRun,
  parseAnswers,
  parseFork,
  parseTools,
} from "@/features/chat/lib/agui/vymalo";
import { drawsPart, isAnswerPart, isStepPart } from "@/features/chat/lib/steps";
import { type TextRole, textRoles } from "@/features/chat/lib/working";
import { MessageBranches } from "@/features/threads/components/branch-picker";
import { ForkDivider } from "@/features/threads/components/fork-divider";
import { useThreadFork } from "@/features/threads/components/fork-provider";
import { ToolsLine } from "@/features/tools/components/tools-line";
import type { ApiActor } from "@/lib/api/types";
import { isActive } from "@/lib/api/types";
import { uuidv7 } from "@/lib/uuid";

/*
 * Pruned from the assistant-ui `thread` registry item and rebuilt as a classical chat
 * (web/DESIGN.md). The person's words are a soft bubble on the right. A run is one assistant
 * message, drawn as a turn: the agent's mark and name once, one summary line that opens the side
 * panel on this turn (the steps themselves, every status, artifact, check, CI report, rework and
 * action, are the panel's Activity tab), then its parts in order: the agent's words are prose, and
 * a failure or a surface stands on its own; the reply the agent is still writing (live text, never
 * in the transcript) is drawn after the parts, with a caret; the pull requests and files it shared
 * follow as cards. A `vymalo.actor` marker part says who ran.
 *
 * The column keeps one answer per turn (ADR 0031): what the agent said while it worked is not drawn
 * here at all, neither collapsed nor behind a control; it is a note among the steps in the panel
 * (`lib/working.ts` says which text is which, `lib/step-tree.ts` files the notes). The runtime's
 * message is whole: the text parts are all in it, the chat only does not draw the working ones.
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

/** Every stretch of step parts is one group, which the chat draws as nothing: the panel has them. */
const byStep = (part: PartState): readonly "group-steps"[] =>
  isStepPart(part as AnyPart) ? ["group-steps"] : [];

/** The `runId` of the run of the log this turn is, from the marker part in front of its output. */
const useRunId = (): string | undefined =>
  useAuiState((s) => {
    const marker = s.message.content.find((p) => p.type === "data" && p.name === ACTOR_PART);
    return marker && marker.type === "data" ? parseActorRun(marker.data) : undefined;
  });

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

/**
 * Where a message of the person is in the log (ADR 0029: an edit and the versions of a message name
 * events by `seq`): the `seq` a replayed message was appended with, else what the page read of the
 * message it sent itself. Undefined until the log has said it.
 */
const useUserSeq = (): number | undefined => {
  const custom = useAuiState((s) => s.message.metadata?.custom?.seq);
  const id = useAuiState((s) => s.message.id);
  const { seqOfMessage } = useThreadFork();
  return typeof custom === "number" ? custom : seqOfMessage(id);
};

/**
 * A message of the person: a soft bubble on the right. Under it, `‹ 2/3 ›` when it has other
 * versions, and, on hover and on focus (always on a touch screen), **Edit**: the bubble becomes an
 * editor, and sending it makes a new branch of the conversation from this message (a new chat the
 * page goes to; the old words and their answers stay in the other version). `id="m-<seq>"` and
 * `data-seq` name the message in the log, which is where a version's link scrolls to.
 */
export const UserMessage: FC = () => {
  const createdAt = useCreatedAt();
  const seq = useUserSeq();
  const fork = useThreadFork();
  const text = useAuiState((s) =>
    s.message.content.flatMap((p) => (p.type === "text" ? [p.text] : [])).join("\n\n"),
  );
  const [editing, setEditing] = useState(false);
  // one id for the message this edit makes, so a retry of the same edit is the same request
  const messageId = useRef("");
  const editButton = useRef<HTMLButtonElement>(null);
  const editable = fork.available && seq !== undefined;
  const stop = () => {
    setEditing(false);
    // the button the editor replaced is where the person was
    requestAnimationFrame(() => editButton.current?.focus());
  };
  return (
    <MessagePrimitive.Root
      data-slot="user-message"
      data-role="user"
      {...(seq !== undefined ? { id: `m-${seq}`, "data-seq": seq } : {})}
      // focusable by the program only: arriving at a version puts the focus on its message
      tabIndex={-1}
      className="group/user flex min-w-0 motion-safe:animate-turn-in flex-col items-end outline-none"
    >
      {editing && seq !== undefined ? (
        <MessageEditor
          initialText={text}
          busy={fork.busy}
          onCancel={stop}
          onSend={(next) => {
            void fork.editMessage(seq, next, messageId.current);
          }}
        />
      ) : (
        <WithTime at={createdAt}>
          <div className="max-w-[85%] rounded-[20px] rounded-tr-md bg-bubble px-4 py-2.5 text-[0.9375rem] leading-6 [&_.aui-md-p]:leading-6 [overflow-wrap:anywhere] sm:max-w-[80%] [&_.aui-md-inline-code]:bg-background/70">
            <MessagePrimitive.Parts components={{ Text: MarkdownText }} />
          </div>
        </WithTime>
      )}
      {editing ? null : <DeliveryNote />}
      {editing ? null : (
        <div className="mt-0.5 flex items-center gap-0.5 text-muted-foreground">
          <MessageBranches seq={seq} />
          {editable ? (
            <TooltipIconButton
              ref={editButton}
              tooltip="Edit"
              aria-label="Edit what you said"
              data-slot="edit-message"
              side="bottom"
              className="size-7 rounded-md p-0 transition-opacity aria-disabled:opacity-50 group-focus-within/user:opacity-100 group-hover/user:opacity-100 focus-visible:opacity-100 [@media(hover:hover)]:opacity-0 [&_svg]:size-3.5"
              // not `disabled`: a disabled button takes no focus, and a fork is being made
              aria-disabled={fork.busy}
              onClick={() => {
                if (fork.busy) return;
                messageId.current = uuidv7();
                setEditing(true);
              }}
            >
              <PencilIcon aria-hidden="true" />
            </TooltipIconButton>
          ) : null}
        </div>
      )}
    </MessagePrimitive.Root>
  );
};

/** The agent's avatar and name, once per turn: `coder · coder-r47` (the name, then the revision). */
function TurnHeader({ actor, at }: { actor: ApiActor | undefined; at?: Date | undefined }) {
  const { agentId } = useThreadView();
  const name = actor?.name ?? agentId;
  return (
    // focusable by the program only: the panel's "Turn n" puts the focus here
    <div data-slot="turn-header" tabIndex={-1} className="flex min-w-0 items-center gap-2.5">
      <AgentAvatar agentId={agentId ?? name ?? "agent"} name={name ?? "Agent"} />
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

/**
 * One text part of the turn: the agent's words when they are its answer, nothing when they are
 * working text (a note of the panel's steps). The part does not know its own place, so the index
 * comes from the runtime's part accessor; a part that cannot say is drawn, as words always were.
 */
const TextLeaf: FC<{
  roles: ReadonlyMap<number, TextRole>;
  /** The index of the last part with words, which a waiting turn asks its question in. */
  lastText: number;
  asking: boolean;
}> = ({ roles, lastText, asking }) => {
  const aui = useAui();
  const where = aui.part.query as { type?: string; index?: number } | undefined;
  const index = where?.type === "index" ? where.index : undefined;
  if (index !== undefined && roles.get(index) === "working") return null;
  return <AgentText question={asking && index !== undefined && index === lastText} />;
};

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
  const runId = useRunId();
  const createdAt = useCreatedAt();
  const { waiting, agentId } = useThreadView();
  const content = useAuiState((s) => s.message.content) as readonly AnyPart[];
  const running = useAuiState((s) => s.message.status?.type === "running");
  const isLast = useAuiState((s) => s.message.isLast);
  const messageId = useAuiState((s) => s.message.id);
  // the reply being written belongs to the run's message: the newest one of the agent
  const newest = useAuiState((s) => {
    const all = s.thread.messages;
    for (let i = all.length - 1; i >= 0; i--) {
      if (all[i]?.role === "assistant") return all[i]?.id === s.message.id;
    }
    return false;
  });
  const drafts = useLiveDrafts();

  let lastDrawn = -1;
  let lastText = -1;
  content.forEach((p, i) => {
    if (drawsPart(p)) lastDrawn = i;
    if (p.type === "text" && p.text?.trim()) lastText = i;
  });
  // what the person answered through a Choices: their words, so above the agent's mark, not a step
  const answers = content.flatMap((p, i) => {
    const data = isAnswerPart(p) ? parseAnswers(p.data) : null;
    return data ? [{ i, data }] : [];
  });
  // what the person attached or detached (ADR 0024): their own act, a muted line above the turn
  const toolsPart = activityPartName(ACTIVITY.tools);
  const tools = content.flatMap((p) => {
    const data = p.type === "data" && p.name === toolsPart ? parseTools(p.data) : null;
    return data ? [data] : [];
  });
  // the marker of a fork (ADR 0029) is a run of its own: a divider, not a turn
  const marker = content.find(
    (p) => p.type === "data" && p.name === activityPartName(ACTIVITY.fork),
  );
  if (marker) {
    const fork = parseFork(marker.data);
    return fork ? <ForkDivider fork={fork} /> : null;
  }
  if (lastDrawn < 0 && !running && answers.length === 0 && tools.length === 0) return null;
  // the answer and the working text of the turn (ADR 0031); Copy takes the answer
  const roles = textRoles(content, running);
  const ownWords = content
    .flatMap((p, i) =>
      p.type === "text" && p.text?.trim() && roles.get(i) !== "working" ? [p.text] : [],
    )
    .join("\n\n");
  const writing = newest
    ? drawnDrafts(
        drafts,
        content.flatMap((p) => (p.type === "text" && p.text ? [p.text] : [])),
      )
    : [];
  const answered = (
    <>
      {answers.map(({ i, data }) => (
        <AnswerBubble key={i} data={data} />
      ))}
      {tools.map((data) => (
        <ToolsLine
          key={`${data.at ?? ""}:${data.attached?.join() ?? ""}:${data.detached?.join() ?? ""}`}
          content={data}
        />
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
      data-turn-id={messageId}
      className="group/turn flex min-w-0 motion-safe:animate-turn-in flex-col gap-3"
    >
      {answers.length > 0 || tools.length > 0 ? (
        <div className="mb-3 flex flex-col gap-3">{answered}</div>
      ) : null}
      <TurnHeader actor={actor} at={createdAt} />
      <div className="flex min-w-0 flex-col gap-4 sm:pl-10">
        <TurnSummaryLine turnId={messageId} />
        <MessagePrimitive.GroupedParts groupBy={byStep} indicator="always">
          {({ part }) => {
            switch (part.type) {
              case "group-steps":
                // the steps are the panel's Activity tab; the line above opens it on this turn
                return null;
              case "text":
                return <TextLeaf roles={roles} lastText={lastText} asking={waiting && isLast} />;
              case "data":
                // the person's answers are drawn above the turn
                if (isAnswerPart(part as AnyPart)) return null;
                // what was attached is drawn above the turn, with the person's other acts
                if ((part as AnyPart).name === toolsPart) return null;
                return <div className="w-full empty:hidden">{part.dataRendererUI}</div>;
              case "indicator":
                // the turn's line says it works; before its first event there is only this
                return lastDrawn < 0 && writing.length === 0 ? (
                  <Starting name={actor?.name ?? agentId} />
                ) : null;
              default:
                return null;
            }
          }}
        </MessagePrimitive.GroupedParts>
        {writing.map((d) => (
          <LiveDraft key={d.id} id={d.id} text={d.text} />
        ))}
        <TurnCards />
        <TurnActions text={ownWords} runId={runId} last={isLast} />
      </div>
    </MessagePrimitive.Root>
  );
};
