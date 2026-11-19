"use client";

import { CheckIcon, CopyIcon, RefreshCwIcon, SplitIcon, TriangleAlertIcon } from "lucide-react";
import { type FC, useId } from "react";
import { TooltipIconButton } from "@/components/assistant-ui/elements/tooltip-icon-button";
import { QuoteButton } from "@/features/chat/components/quote";
import { ReadAloud } from "@/features/speech/components/read-aloud";
import { useThreadFork } from "@/features/threads/components/fork-provider";
import { COPY_FAILED, useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";

/** What "Fork from here" says while the turn it would copy is still going on. */
export const FORK_WAIT = "Fork from here (once the agent has finished this turn)";

/** What "Regenerate" says while the turn it would say again is still going on. */
export const REGENERATE_WAIT = "Regenerate (once the agent has finished this turn)";

type Props = {
  /** The agent's words in this turn, for Copy and Quote: its source text, with one line for each surface it drew. */
  text: string;
  /** The `runId` of the log run this turn is (the marker part says it): where the turn ends in the log. */
  runId: string | undefined;
  /** This is the newest turn of the thread: the one that is going on while the thread works. */
  last: boolean;
  /**
   * The message of the person that began this turn, when it can be said again: its `seq` in the log and
   * its words. Absent when the turn began with a form answer, a mention (an edit carries none) or
   * before the page has read the message's place in the log.
   */
  began?: { seq: number; text: string } | undefined;
};

/**
 * What a person can do with a finished agent turn: copy its words (the Markdown source as `text/plain`, and a failure said in words), quote them into the composer, read them aloud with the device's voice, and **fork from here**, which
 * makes a new chat that holds the conversation up to the end of this turn (ADR 0029). They show
 * on hover and on focus (always on a touch screen, which has no hover), under the turn.
 *
 * **Regenerate** is on the newest turn only: an edit of the message that began it with the same
 * words (ADR 0029, amended 2026-11-14), so the answer is a new version and `‹ 2/2 ›` goes between
 * them. It is not offered to a person who may not write in the thread, and not on a shared link.
 *
 * Fork is disabled while the turn is going on: nothing of a turn that is not over can be copied
 * (the server says `turn_open`), and it is not offered before it is known where the turn ends.
 */
export const TurnActions: FC<Props> = ({ text, runId, last, began }) => {
  const fork = useThreadFork();
  const { isCopied, failed, copyToClipboard } = useCopyToClipboard();
  // Read aloud is one answer at a time across the page: this is what tells this answer from the others
  const ownId = useId();
  const forkable = fork.available && fork.canForkRun(runId);
  const open = fork.turnOpen && last;
  const unavailable = open || fork.busy;
  const regenerable = fork.available && last && began !== undefined;
  if (!text && !forkable && !regenerable) return null;
  return (
    <div
      data-slot="turn-actions"
      className="-ms-1.5 flex items-center gap-0.5 text-muted-foreground transition-opacity [@media(hover:hover)]:opacity-0 group-focus-within/turn:opacity-100 group-hover/turn:opacity-100"
    >
      {text ? (
        <>
          <TooltipIconButton
            tooltip={failed ? COPY_FAILED : isCopied ? "Copied" : "Copy"}
            aria-label="Copy"
            side="bottom"
            className="size-7 rounded-md p-0 [&_svg]:size-3.5"
            onClick={() => copyToClipboard(text)}
          >
            {failed ? (
              <TriangleAlertIcon aria-hidden="true" />
            ) : isCopied ? (
              <CheckIcon aria-hidden="true" />
            ) : (
              <CopyIcon aria-hidden="true" />
            )}
          </TooltipIconButton>
          <span aria-live="polite" data-slot="copy-note" className={failed ? "text-xs" : "sr-only"}>
            {failed ? COPY_FAILED : ""}
          </span>
          <QuoteButton source={text} />
          {/* two id spaces, so a run's id and a React id cannot meet: `run:` from the log, `own:` from this page */}
          <ReadAloud id={runId ? `run:${runId}` : `own:${ownId}`} text={text} />
        </>
      ) : null}
      {regenerable && began ? (
        <TooltipIconButton
          tooltip={open ? REGENERATE_WAIT : fork.busy ? "Regenerating…" : "Regenerate"}
          aria-label="Regenerate"
          data-slot="regenerate"
          aria-disabled={unavailable}
          side="bottom"
          className="size-7 rounded-md p-0 aria-disabled:opacity-50 [&_svg]:size-3.5"
          onClick={() => {
            if (!unavailable) void fork.regenerate(began.seq, began.text);
          }}
        >
          <RefreshCwIcon aria-hidden="true" />
        </TooltipIconButton>
      ) : null}
      {forkable && runId !== undefined ? (
        <TooltipIconButton
          tooltip={open ? FORK_WAIT : "Fork from here"}
          aria-label="Fork from here"
          // not `disabled`: a disabled button takes no focus, and its tooltip says why
          aria-disabled={unavailable}
          side="bottom"
          className="size-7 rounded-md p-0 aria-disabled:opacity-50 [&_svg]:size-3.5"
          onClick={() => {
            if (!unavailable) fork.forkRun(runId);
          }}
        >
          <SplitIcon aria-hidden="true" />
        </TooltipIconButton>
      ) : null}
    </div>
  );
};
