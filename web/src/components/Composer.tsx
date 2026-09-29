import { ComposerPrimitive } from "@assistant-ui/react";
import Link from "next/link";
import type { ThreadState } from "@/api/types";
import { isActive, isTerminal } from "@/api/types";
import { InlineStatus } from "./InlineStatus";

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
    <div className="composer-area">
      {blocked ? (
        <p className="notice" role="status">
          <strong>Waiting for your answer.</strong>
          {lastQuestion ? <span> {lastQuestion}</span> : null}
        </p>
      ) : null}
      {finished ? (
        <p className="notice" role="status">
          This thread is {state}. <Link href="/">Start a new thread</Link> to continue.
        </p>
      ) : null}
      {sendError ? (
        <InlineStatus tone="error" role="alert">
          {sendError}
        </InlineStatus>
      ) : null}
      <ComposerPrimitive.Root className="composer">
        <ComposerPrimitive.Input
          className="composer__input"
          aria-label="Message"
          placeholder={placeholder}
          rows={1}
          maxRows={8}
          cancelOnEscape={false}
        />
        {running ? (
          <ComposerPrimitive.Cancel className="btn btn--secondary">Cancel</ComposerPrimitive.Cancel>
        ) : (
          <ComposerPrimitive.Send className="btn btn--primary">Send</ComposerPrimitive.Send>
        )}
      </ComposerPrimitive.Root>
    </div>
  );
}
