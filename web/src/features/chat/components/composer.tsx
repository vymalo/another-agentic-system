import { ComposerPrimitive, isMessageNotSentError, useAui, useAuiState } from "@assistant-ui/react";
import { useAgUiInterrupts, useAgUiSteerAway } from "@assistant-ui/react-ag-ui";
import Link from "next/link";
import type { FormEvent, RefObject } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import type { ThreadState } from "@/lib/api/types";
import { isActive, isTerminal } from "@/lib/api/types";

type Props = {
  /** The thread's state; undefined until known. */
  state: ThreadState | undefined;
  /** The new-thread page: no thread yet. */
  isNew: boolean;
  sendError: string | null;
  /** Ask the orchestrator to cancel (`POST /api/threads/{id}/cancel`). */
  onCancel: () => void;
  /** The textarea, so an A2UI `userMessage` can focus it. */
  inputRef?: RefObject<HTMLTextAreaElement | null>;
};

/**
 * The message box. Sending is the runtime's: a message starts a run (`POST /agui/agents/{id}`).
 * Two things are ours: while an interrupt waits, the text is the interrupt's answer (a run that
 * `resume`s it), and Cancel asks the orchestrator, because the runtime's own cancel only detaches
 * (AG-UI: a consumer that leaves has a truncated run, not a cancelled one).
 */
export function Composer({ state, isNew, sendError, onCancel, inputRef }: Props) {
  const aui = useAui();
  const interrupts = useAgUiInterrupts();
  const steerAway = useAgUiSteerAway();
  const composerEmpty = useAuiState((s) => s.composer.isEmpty);
  const running = isActive(state);
  const finished = !isNew && isTerminal(state);
  const blocked = state === "blocked";
  const interrupt = interrupts[0];
  const question = interrupt
    ? interrupt.reason === "auth_required"
      ? interrupt.message
        ? `Authentication required: ${interrupt.message}`
        : "Authentication required"
      : interrupt.message
    : undefined;
  const placeholder = isNew
    ? "Describe the task…"
    : blocked
      ? "Answer the agent…"
      : running
        ? "The agent is working…"
        : finished
          ? "This thread is finished"
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

  return (
    <div className="flex flex-col gap-2 bg-background pt-3 pb-4">
      {blocked ? (
        <Alert role="status">
          <AlertTitle>Waiting for your answer.</AlertTitle>
          {question ? <AlertDescription>{question}</AlertDescription> : null}
        </Alert>
      ) : null}
      {finished ? (
        <Alert role="status">
          <AlertDescription>
            This thread is {state}. <Link href="/">Start a new thread</Link> to continue.
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
        className="flex items-end gap-2 rounded-xl border border-input bg-background p-2 transition-[border-color,box-shadow] focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50"
      >
        <ComposerPrimitive.Input
          ref={inputRef}
          className="max-h-48 min-h-10 min-w-0 flex-1 resize-none bg-transparent px-2 py-2 text-base outline-none placeholder:text-muted-foreground disabled:cursor-not-allowed"
          aria-label="Message"
          placeholder={placeholder}
          rows={1}
          maxRows={8}
          cancelOnEscape={false}
          disabled={finished}
        />
        {running ? (
          <Button type="button" variant="outline" className="h-10 px-5" onClick={onCancel}>
            Cancel
          </Button>
        ) : interrupts.length > 0 ? (
          // not ComposerPrimitive.Send: its click would also send the text as a plain message
          <Button type="submit" className="h-10 px-5" disabled={composerEmpty}>
            Send
          </Button>
        ) : (
          <ComposerPrimitive.Send asChild>
            <Button type="submit" className="h-10 px-5">
              Send
            </Button>
          </ComposerPrimitive.Send>
        )}
      </ComposerPrimitive.Root>
    </div>
  );
}
