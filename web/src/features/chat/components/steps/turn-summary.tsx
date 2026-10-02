"use client";

import { ChevronRightIcon } from "lucide-react";
import { memo } from "react";
import { useTurnSteps } from "@/features/chat/hooks/use-turn-steps";
import { firstFailed, summaryLine, type TurnSteps } from "@/features/chat/lib/step-tree";
import { useStepsPanel } from "@/features/panel/hooks/use-steps-panel";
import { cn } from "@/lib/utils";
import { FailedChipButton } from "./step-node";
import { TurnGlyph } from "./turn-glyph";

const capitalise = (name: string): string => name.charAt(0).toUpperCase() + name.slice(1);

/**
 * The one line an agent's turn keeps in the chat: what it is on while it runs, how much it did
 * when it is done, a failure chip whenever a step failed, even in a turn that went well. The
 * steps themselves are in the side panel; this opens it on this turn, and its failed chip opens
 * it on the first step that failed, with that step's input and output. Nothing for a turn of words
 * only. Prop-driven: `panelShowsTurn` and `onOpen` come from the shell (`useStepsPanel`).
 */
export const TurnSummary = memo(function TurnSummary({
  turn,
  panelShowsTurn,
  onOpen,
  panelId = "thread-panel",
}: {
  turn: TurnSteps;
  panelShowsTurn: boolean;
  onOpen(turnId: string, stepId?: string): void;
  panelId?: string;
}) {
  const line = summaryLine(turn);
  if (!line) return null;
  const who = capitalise(turn.agent.name);
  const name = `${who}'s steps: ${line.text}${line.failed > 0 ? `, ${line.failed} failed` : ""}. Show in the side panel`;
  const failed = line.failed > 0 ? firstFailed(turn) : undefined;
  return (
    // the chip is a control of its own, so it sits beside the line's button and not inside it
    <span
      data-slot="turn-summary-line"
      className={cn(
        "group/summary -ms-2 inline-flex h-7 max-w-full min-w-0 items-center self-start rounded-full text-[0.8125rem] leading-5 text-muted-foreground",
        "hover:bg-muted hover:text-foreground",
        panelShowsTurn && "bg-muted text-foreground",
      )}
    >
      <button
        type="button"
        data-slot="turn-summary"
        data-state={turn.state}
        aria-label={name}
        aria-controls={panelId}
        aria-expanded={panelShowsTurn}
        onClick={() => onOpen(turn.turnId)}
        className="inline-flex h-7 min-w-0 cursor-pointer items-center gap-2 rounded-full ps-2 pe-2 focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
      >
        <TurnGlyph icon={line.icon} />
        <span
          className={cn("min-w-0 truncate", line.icon === "spinner" && "text-shimmer")}
          data-slot="turn-summary-text"
        >
          {line.text}
        </span>
        <ChevronRightIcon
          aria-hidden="true"
          className="size-3.5 shrink-0 opacity-60 group-hover/summary:opacity-100"
        />
      </button>
      {line.failed > 0 ? (
        <span className="pe-1.5">
          <FailedChipButton count={line.failed} onClick={() => onOpen(turn.turnId, failed?.id)} />
        </span>
      ) : null}
    </span>
  );
});

/** The summary of one turn of the thread the page is in, wired to the side panel. */
export function TurnSummaryLine({ turnId }: { turnId: string }) {
  const turns = useTurnSteps();
  const panel = useStepsPanel();
  const turn = turns.find((t) => t.turnId === turnId);
  if (!turn) return null;
  return (
    <TurnSummary
      turn={turn}
      panelId={panel.panelId}
      panelShowsTurn={panel.open && panel.tab === "activity" && panel.focus?.turnId === turnId}
      onOpen={panel.openSteps}
    />
  );
}
