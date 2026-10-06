import { ComposerPrimitive, isMessageNotSentError, useAui, useAuiState } from "@assistant-ui/react";
import { useAgUiInterrupts, useAgUiSteerAway } from "@assistant-ui/react-ag-ui";
import { ArrowUpIcon, SquareIcon } from "lucide-react";
import {
  type FormEvent,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
  useRef,
  useState,
} from "react";
import { InlineStatus } from "@/components/inline-status";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { SendError } from "@/features/chat/lib/agui/thread-agent";
import type { JobView, SendMode } from "@/features/chat/lib/agui/vymalo";
import { modeOfKey } from "@/features/chat/lib/send";
import { MentionChips } from "@/features/mentions/components/mention-chips";
import { MentionListbox } from "@/features/mentions/components/mention-listbox";
import { useMentions } from "@/features/mentions/hooks/use-mentions";
import { MentionsStore } from "@/features/mentions/lib/store";
import type { ApiAgent, ThreadState } from "@/lib/api/types";
import { isActive, isTerminal } from "@/lib/api/types";
import { cn } from "@/lib/utils";
import { SendSplit } from "./send-split";

type Props = {
  /** The thread's state; undefined until known. */
  state: ThreadState | undefined;
  /** Where the job stands under a verification gate; null without one. */
  job?: JobView | null;
  /** The `RUN_ERROR` that ended the newest run, if it failed: why the thread is `failed`. */
  failure?: { code: string; message: string } | null;
  /** The new-thread page: no thread yet. */
  isNew: boolean;
  sendError: string | null;
  /** Ask the orchestrator to cancel (`POST /api/threads/{id}/cancel`). */
  onCancel: () => void;
  /** The textarea, so an A2UI `userMessage` can focus it. */
  inputRef?: RefObject<HTMLTextAreaElement | null>;
  /**
   * The left of the box's bottom row: the tools picker and its chips (`features/tools`); the chips of
   * the mentions follow them. The agent is picked in the top bar (agent-menu.tsx).
   */
  toolbar?: ReactNode;
  /**
   * What is true of the message before it is sent, as lines above the box: the agent cannot use the
   * attached tools, a change of them was refused. The send error is the last of them.
   */
  notices?: ReactNode;
  /**
   * Mentioning agents (ADR 0026): the agents the person may mention here (invokable, and not the
   * agent that reads the message), and the store that keeps the mentions of the text in the box
   * (`features/mentions`). Absent: no "@" opens anything.
   */
  mentions?: {
    agents: readonly ApiAgent[];
    store: MentionsStore;
  };
  /**
   * Sending while the agent works (ADR 0036). Absent: the box does not offer it (the thread's agent
   * is not known yet).
   */
  sending?: {
    /** `ThreadAgent.sendWhileWorking`: resolves when the orchestrator accepted the message. */
    send: (text: string, mode: SendMode) => Promise<void>;
    /**
     * A message that was refused: what the orchestrator said (the text is back in the box), and its
     * HTTP status when it answered (a 422 or a 503 has the agent list read again).
     */
    onFailed: (message: string, status?: number) => void;
    /** Who works, for the menu ("Adam reads it at its next step"). */
    agent: string;
    /** Whether the agent's card lists `steer/v1`; null when it could not be read. */
    steers: boolean | null;
    /**
     * The conversation is on screen (the replay has been applied): a person writes to what they
     * have seen, and the runs the stream delivered are in the transcript before the message is.
     */
    ready: boolean;
  };
};

/** `RUN_ERROR.code` of a job whose last attempt did not pass the verification gate (ADR 0018). */
export const CHECKS_FAILED = "checks_failed";

const attempts = (n: number): string => `${n} ${n === 1 ? "attempt" : "attempts"}`;

/**
 * The message box. Sending is the runtime's: a message starts a run (`POST /agui/agents/{id}`).
 * Three things are ours: while an interrupt waits, the text is the interrupt's answer (a run that
 * `resume`s it); Stop asks the orchestrator, because the runtime's own cancel only detaches (AG-UI:
 * a consumer that leaves has a truncated run, not a cancelled one); and while the agent works the
 * box stays open and a message is sent with `vymalo.send` (ADR 0036): **Send** (Enter) steers, and
 * **Stop and send** (Ctrl/⌘+Shift+Enter, or the menu beside Send) interrupts. Stop is always there.
 * That send does not go through the runtime, whose own send would end the run it is showing as
 * cancelled (`ThreadAgent.sendWhileWorking` says why): the message comes back by the stream.
 */
export function Composer({
  state,
  job,
  failure,
  isNew,
  sendError,
  onCancel,
  inputRef,
  toolbar,
  notices,
  mentions,
  sending,
}: Props) {
  const aui = useAui();
  const interrupts = useAgUiInterrupts();
  const steerAway = useAgUiSteerAway();
  const composerEmpty = useAuiState((s) => s.composer.isEmpty);
  // "@" in the box opens the agents that may be mentioned, unless the box is an answer (a resume has no message to carry them)
  const [ownStore] = useState(() => new MentionsStore());
  const ownRef = useRef<HTMLTextAreaElement | null>(null);
  const boxRef = inputRef ?? ownRef;
  const store = mentions?.store ?? ownStore;
  const running = isActive(state);
  const finished = !isNew && isTerminal(state);
  const blocked = state === "blocked";
  // A thread never locks (ADR 0020): the box is always there. What the agent says next is
  // the placeholder's business, not a reason to disable it.
  const placeholder = isNew
    ? "Describe a task for the agent…"
    : blocked
      ? "Reply…"
      : state === "failed" || state === "cancelled"
        ? "Tell the agent how to go on…"
        : "Send a follow-up…";

  // While the agent works (and does not wait for an answer) a message goes out with how it is
  // delivered; the runtime's own send would be a 409 on an open run.
  const whileWorking = running && interrupts.length === 0 && sending !== undefined;
  const mention = useMentions({
    agents: mentions?.agents ?? [],
    store,
    inputRef: boxRef,
    enabled: mentions !== undefined && interrupts.length === 0,
  });
  const sendWhileWorking = (mode: SendMode) => {
    if (!sending?.ready) return;
    const composer = aui.composer();
    const text = composer.getState().text.trim();
    if (!text) return;
    // what the message mentions, before the box is emptied: a refused one gets it back with its words
    const taken = store.take(text);
    composer.setText("");
    sending.send(text, mode).catch((e: unknown) => {
      // refused: nothing of it reached the log, so the words come back in front of anything written since,
      // with the agents they mention
      const since = composer.getState().text;
      composer.setText(store.restoreInFront(text, taken, since));
      if (e instanceof SendError) sending.onFailed(e.message, e.status);
      else sending.onFailed(e instanceof Error ? e.message : String(e));
    });
    focusBox();
  };
  // the button that was pressed is gone with the text: the box is where the person is
  const focusBox = () => {
    requestAnimationFrame(() => boxRef.current?.focus());
  };
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    // the list of agents has the arrows, Enter, Tab and Escape while it is open
    mention.onKeyDown(e);
    if (e.defaultPrevented) return;
    if (!whileWorking || e.nativeEvent.isComposing) return;
    const mode = modeOfKey(e);
    if (!mode) return;
    e.preventDefault();
    sendWhileWorking(mode);
  };

  // With an interrupt open the runtime refuses a plain message; the text answers it instead.
  const answerInterrupt = (e: FormEvent<HTMLFormElement>) => {
    if (whileWorking) {
      // a submit that is not a key we read (Ctrl+Enter): the same as Send
      e.preventDefault();
      sendWhileWorking("steer");
      return;
    }
    if (interrupts.length === 0) return; // an ordinary send: the runtime's own handler
    e.preventDefault();
    const composer = aui.composer();
    const text = composer.getState().text.trim();
    if (!text) return;
    composer.setText("");
    void steerAway(
      { role: "user", content: [{ type: "text", text }] },
      interrupts.map((i, n) =>
        n === 0
          ? { interruptId: i.id, status: "resolved" as const, payload: { text } }
          : { interruptId: i.id, status: "cancelled" as const },
      ),
    ).catch((e: unknown) => {
      // a refused answer is reported through the runtime's onError; the draft comes back
      if (isMessageNotSentError(e)) composer.setText(text);
    });
  };

  const round =
    "size-9 shrink-0 rounded-full p-0 [&_svg:not([class*='size-'])]:size-4.5 disabled:bg-muted disabled:text-muted-foreground disabled:opacity-100";
  return (
    <div className={cn("flex flex-col gap-2", isNew ? "" : "pb-3")}>
      {/* The question is the agent's last message, in the conversation; the box only says that
          the agent waits, for a screen reader. The live region stays mounted and only its text
          changes: one that appears with its text is often not announced. */}
      <p role="status" className="sr-only">
        {blocked ? "Waiting for your answer." : ""}
      </p>
      {finished && state === "failed" && failure?.code === CHECKS_FAILED ? (
        <Alert variant="destructive" role="status" data-slot="checks-failed" className="rounded-xl">
          <AlertTitle>
            {job ? `Checks failed after ${attempts(job.attempt)}` : "Checks failed"}
          </AlertTitle>
          <AlertDescription>
            The agent finished, but its work did not pass verification and no attempts are left. The
            findings are in the side panel, under Activity. Write a message to go on.
          </AlertDescription>
        </Alert>
      ) : null}
      {notices}
      {sendError ? (
        <InlineStatus tone="error" role="alert">
          {sendError}
        </InlineStatus>
      ) : null}
      <ComposerPrimitive.Root
        onSubmit={answerInterrupt}
        data-slot="composer"
        className="relative flex flex-col gap-1 rounded-3xl border border-input bg-card p-2 shadow-composer transition-[border-color,box-shadow] focus-within:border-ring/60 focus-within:ring-4 focus-within:ring-ring/15"
      >
        {mention.open ? (
          <MentionListbox
            id={mention.listboxId}
            optionId={mention.optionId}
            options={mention.options}
            active={mention.active}
            onPick={mention.pick}
            onHover={mention.hover}
          />
        ) : null}
        <ComposerPrimitive.Input
          ref={boxRef}
          {...mention.comboboxProps}
          className="max-h-60 min-h-11 w-full min-w-0 resize-none bg-transparent px-3 pt-2 pb-1 text-base leading-6 outline-none placeholder:text-muted-foreground disabled:cursor-not-allowed"
          aria-label="Message"
          placeholder={placeholder}
          rows={1}
          maxRows={8}
          cancelOnEscape={false}
          // Enter sends. While the agent works `onKeyDown` takes it first (the runtime's own Enter
          // does nothing on an open run), and Ctrl/⌘+Shift+Enter is Stop and send.
          submitMode="enter"
          onKeyDown={onKeyDown}
          onSelect={mention.onSelect}
        />
        <div className="flex min-w-0 items-center gap-2 ps-1">
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">
            {toolbar}
            <MentionChips
              mentions={store.current}
              agents={mentions?.agents ?? []}
              onRemove={mention.remove}
            />
          </div>
          {running ? (
            <>
              <Button
                type="button"
                variant="secondary"
                aria-label="Stop"
                title="Stop the agent"
                className={cn(round, "bg-foreground text-background hover:bg-foreground/85")}
                onClick={onCancel}
              >
                <SquareIcon aria-hidden="true" className="size-3.5 fill-current" />
              </Button>
              {whileWorking && !composerEmpty && sending ? (
                <SendSplit
                  agent={sending.agent}
                  steers={sending.steers}
                  loading={!sending.ready}
                  onSend={sendWhileWorking}
                  onClosed={focusBox}
                />
              ) : null}
            </>
          ) : interrupts.length > 0 ? (
            // not ComposerPrimitive.Send: its click would also send the text as a plain message
            <Button type="submit" aria-label="Send" className={round} disabled={composerEmpty}>
              <ArrowUpIcon aria-hidden="true" />
            </Button>
          ) : (
            <ComposerPrimitive.Send asChild>
              <Button type="submit" aria-label="Send" className={round}>
                <ArrowUpIcon aria-hidden="true" />
              </Button>
            </ComposerPrimitive.Send>
          )}
        </div>
      </ComposerPrimitive.Root>
      {isNew ? null : (
        <p className="px-4 text-center text-xs text-muted-foreground">
          Agents can make mistakes. Check their work.
        </p>
      )}
    </div>
  );
}
