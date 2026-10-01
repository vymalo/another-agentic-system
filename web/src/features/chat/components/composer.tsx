import { ComposerPrimitive, isMessageNotSentError, useAui, useAuiState } from "@assistant-ui/react";
import { useAgUiInterrupts, useAgUiSteerAway } from "@assistant-ui/react-ag-ui";
import { ArrowUpIcon, SquareIcon } from "lucide-react";
import type { FormEvent, ReactNode, RefObject } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import type { JobView } from "@/features/chat/lib/agui/vymalo";
import type { ThreadState } from "@/lib/api/types";
import { isActive, isTerminal } from "@/lib/api/types";
import { cn } from "@/lib/utils";

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
   * The left of the box's bottom row. Empty today: the agent is picked in the top bar
   * (agent-menu.tsx), and this is where the tools picker and mentions will go (plan 05).
   */
  toolbar?: ReactNode;
};

/** `RUN_ERROR.code` of a job whose last attempt did not pass the verification gate (ADR 0018). */
export const CHECKS_FAILED = "checks_failed";

const attempts = (n: number): string => `${n} ${n === 1 ? "attempt" : "attempts"}`;

/**
 * The message box. Sending is the runtime's: a message starts a run (`POST /agui/agents/{id}`).
 * Two things are ours: while an interrupt waits, the text is the interrupt's answer (a run that
 * `resume`s it), and Stop asks the orchestrator, because the runtime's own cancel only detaches
 * (AG-UI: a consumer that leaves has a truncated run, not a cancelled one).
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
}: Props) {
  const aui = useAui();
  const interrupts = useAgUiInterrupts();
  const steerAway = useAgUiSteerAway();
  const composerEmpty = useAuiState((s) => s.composer.isEmpty);
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

  // With an interrupt open the runtime refuses a plain message; the text answers it instead.
  const answerInterrupt = (e: FormEvent<HTMLFormElement>) => {
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
      {sendError ? (
        <InlineStatus tone="error" role="alert">
          {sendError}
        </InlineStatus>
      ) : null}
      <ComposerPrimitive.Root
        onSubmit={answerInterrupt}
        data-slot="composer"
        className="flex flex-col gap-1 rounded-3xl border border-input bg-card p-2 shadow-composer transition-[border-color,box-shadow] focus-within:border-ring/60 focus-within:ring-4 focus-within:ring-ring/15"
      >
        <ComposerPrimitive.Input
          ref={inputRef}
          className="max-h-60 min-h-11 w-full min-w-0 resize-none bg-transparent px-3 pt-2 pb-1 text-base leading-6 outline-none placeholder:text-muted-foreground disabled:cursor-not-allowed"
          aria-label="Message"
          placeholder={placeholder}
          rows={1}
          maxRows={8}
          cancelOnEscape={false}
          // While a run is live the box is for drafting: Enter does not send (the run is open),
          // and the button says Stop. The next message goes once the run has ended.
          submitMode={running ? "none" : "enter"}
        />
        <div className="flex min-w-0 items-center gap-2 ps-1">
          <div className="flex min-w-0 flex-1 flex-wrap items-center gap-1.5">{toolbar}</div>
          {running ? (
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
