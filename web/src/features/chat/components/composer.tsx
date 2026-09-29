import { ComposerPrimitive } from "@assistant-ui/react";
import Link from "next/link";
import { InlineStatus } from "@/components/inline-status";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import type { ThreadState } from "@/lib/api/types";
import { isActive, isTerminal } from "@/lib/api/types";

type Props = {
  /** null on the new-thread page. */
  state: ThreadState | undefined;
  isNew: boolean;
  lastQuestion: string | undefined;
  sendError: string | null;
};

export function Composer({ state, isNew, lastQuestion, sendError }: Props) {
  const running = isActive(state);
  const finished = !isNew && isTerminal(state);
  const blocked = state === "blocked";
  const placeholder = isNew
    ? "Describe the task…"
    : blocked
      ? "Answer the agent…"
      : running
        ? "The agent is working…"
        : finished
          ? "This thread is finished"
          : "Send a follow-up…";

  return (
    <div className="flex flex-col gap-2 bg-background pt-3 pb-4">
      {blocked ? (
        <Alert role="status">
          <AlertTitle>Waiting for your answer.</AlertTitle>
          {lastQuestion ? <AlertDescription>{lastQuestion}</AlertDescription> : null}
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
      <ComposerPrimitive.Root className="flex items-end gap-2 rounded-xl border border-input bg-background p-2 transition-[border-color,box-shadow] focus-within:border-ring focus-within:ring-3 focus-within:ring-ring/50">
        <ComposerPrimitive.Input
          className="max-h-48 min-h-10 min-w-0 flex-1 resize-none bg-transparent px-2 py-2 text-base outline-none placeholder:text-muted-foreground disabled:cursor-not-allowed"
          aria-label="Message"
          placeholder={placeholder}
          rows={1}
          maxRows={8}
          cancelOnEscape={false}
        />
        {running ? (
          <ComposerPrimitive.Cancel asChild>
            <Button type="button" variant="outline" className="h-10 px-5">
              Cancel
            </Button>
          </ComposerPrimitive.Cancel>
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
