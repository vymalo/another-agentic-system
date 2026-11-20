"use client";

import { useState } from "react";
import { InlineStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useCloseSession } from "../hooks/use-close-session";
import type { SessionCard } from "../lib/cards";
import type { AppliedClose } from "../lib/children";

/** Where the focus lands when the row that held Close is gone: the Subagents tab's panel, else its New button. */
function stillThere(): HTMLElement | null {
  const panel = document.querySelector<HTMLElement>('[data-slot="sessions-panel"]');
  const tab = panel?.closest<HTMLElement>('[role="tabpanel"]');
  if (tab) return tab;
  return panel?.querySelector<HTMLElement>("button:not([disabled])") ?? null;
}

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The subagent to close: its child thread is `threadId`. */
  card: SessionCard | null;
  /** The close was accepted: what the orchestrator's record says now, and whether it stopped the round. */
  onClosed: (child: string, state: AppliedClose, cancel: boolean) => void;
  returnTo: { current: HTMLElement | null };
};

/**
 * Closing a subagent (`closeChildSession`). An idle one closes at once; one that works has two ways to
 * end, either of which is one button: when its round ends, or now (the round is stopped first). The
 * answer is 202 with the state the record has, which the panel applies before the rail says it.
 */
export function CloseSessionDialog({ open, onOpenChange, card, onClosed, returnTo }: Props) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        onCloseAutoFocus={(event) => {
          // what opened the dialog, else, when a close folded its row away, the tab it was in
          const target = returnTo.current?.isConnected ? returnTo.current : stillThere();
          if (target) {
            event.preventDefault();
            target.focus();
          }
        }}
      >
        {card?.threadId ? (
          <Body card={card} child={card.threadId} onOpenChange={onOpenChange} onClosed={onClosed} />
        ) : null}
      </DialogContent>
    </Dialog>
  );
}

function Body({
  card,
  child,
  onOpenChange,
  onClosed,
}: {
  card: SessionCard;
  child: string;
  onOpenChange: (open: boolean) => void;
  onClosed: Props["onClosed"];
}) {
  const { close, busy } = useCloseSession();
  const [refused, setRefused] = useState<string | null>(null);
  const working = card.state === "working";
  // a close already waits for the round to end: the one thing left to offer is the stronger one
  const waiting = card.closing || card.closeAsked;
  const name = card.agent || "subagent";

  async function go(cancel: boolean) {
    if (busy) return;
    const result = await close(child, cancel);
    if (result.ok) {
      onClosed(child, result.state, cancel);
      onOpenChange(false);
    } else {
      setRefused(result.words);
    }
  }

  return (
    <>
      <DialogHeader>
        <DialogTitle>Close subagent</DialogTitle>
        <DialogDescription>
          Close <em className="not-italic font-medium text-foreground">{name}</em>? It takes no more
          messages.
          {working
            ? waiting
              ? " It closes when its round ends."
              : " It is working on a round now."
            : ""}
        </DialogDescription>
      </DialogHeader>
      {refused ? (
        <InlineStatus tone="error" role="alert">
          {refused}
        </InlineStatus>
      ) : null}
      <DialogFooter>
        <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
          Cancel
        </Button>
        {working ? (
          <>
            {waiting ? null : (
              <Button
                type="button"
                variant="outline"
                disabled={busy}
                onClick={() => void go(false)}
              >
                Close when this round ends
              </Button>
            )}
            <Button
              type="button"
              variant="destructive"
              disabled={busy}
              onClick={() => void go(true)}
            >
              Stop and close
            </Button>
          </>
        ) : (
          <Button type="button" disabled={busy} onClick={() => void go(false)}>
            Close subagent
          </Button>
        )}
      </DialogFooter>
    </>
  );
}
