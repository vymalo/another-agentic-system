"use client";

import { CheckIcon, CopyIcon, SplitIcon } from "lucide-react";
import type { FC } from "react";
import { TooltipIconButton } from "@/components/assistant-ui/elements/tooltip-icon-button";
import { useThreadFork } from "@/features/threads/components/fork-provider";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";

/** What "Fork from here" says while the turn it would copy is still going on. */
export const FORK_WAIT = "Fork from here (once the agent has finished this turn)";

type Props = {
  /** The agent's words in this turn, for Copy. */
  text: string;
  /** The `runId` of the log run this turn is (the marker part says it): where the turn ends in the log. */
  runId: string | undefined;
  /** This is the newest turn of the thread: the one that is going on while the thread works. */
  last: boolean;
};

/**
 * What a person can do with a finished agent turn: copy its words, and **fork from here**, which
 * makes a new chat that holds the conversation up to the end of this turn (ADR 0029). They show
 * on hover and on focus (always on a touch screen, which has no hover), under the turn.
 *
 * Fork is disabled while the turn is going on: nothing of a turn that is not over can be copied
 * (the server says `turn_open`), and it is not offered before it is known where the turn ends.
 */
export const TurnActions: FC<Props> = ({ text, runId, last }) => {
  const fork = useThreadFork();
  const { isCopied, copyToClipboard } = useCopyToClipboard();
  const forkable = fork.available && fork.canForkRun(runId);
  const open = fork.turnOpen && last;
  const unavailable = open || fork.busy;
  if (!text && !forkable) return null;
  return (
    <div
      data-slot="turn-actions"
      className="-ms-1.5 flex items-center gap-0.5 text-muted-foreground transition-opacity [@media(hover:hover)]:opacity-0 group-focus-within/turn:opacity-100 group-hover/turn:opacity-100"
    >
      {text ? (
        <TooltipIconButton
          tooltip={isCopied ? "Copied" : "Copy"}
          aria-label="Copy"
          side="bottom"
          className="size-7 rounded-md p-0 [&_svg]:size-3.5"
          onClick={() => copyToClipboard(text)}
        >
          {isCopied ? <CheckIcon aria-hidden="true" /> : <CopyIcon aria-hidden="true" />}
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
