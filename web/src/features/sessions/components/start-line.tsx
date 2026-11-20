"use client";

import { BotIcon } from "lucide-react";
import Link from "next/link";
import { useThreadView } from "@/features/chat/components/thread-view";
import { parseSessionStart } from "../lib/cards";
import { isHomeCard, threadHref } from "../lib/words";

/**
 * The muted line of a start a person asked for ("You started planner: plan the release"), above the
 * turn, in the voice of the person's other acts (`ToolsLine`): Starting…, then Open, or why it did not
 * start. It is said again in the run that answers it; it is drawn only in its home run (`home`), with
 * the state the stream said last. The agent's words (the name, the description, the reason) are text.
 * "Try again" is the caller's: the dialog that makes the start is not this line's.
 */
export function StartLine({ data, onTryAgain }: { data: unknown; onTryAgain?: () => void }) {
  const { sessionStarts } = useThreadView();
  const own = isHomeCard(data) ? parseSessionStart(data) : null;
  if (!own) return null;
  const card = sessionStarts?.get(own.request) ?? own;
  const href = card.state === "opened" ? threadHref(card.threadId) : undefined;
  return (
    <p
      data-slot="start-line"
      data-testid="start-line"
      data-state={card.state}
      className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 text-[0.8125rem] text-muted-foreground"
    >
      <BotIcon aria-hidden="true" className="size-3.5 shrink-0" />
      <span className="min-w-0 [overflow-wrap:anywhere]">
        You started <em className="not-italic font-medium text-foreground/80">{card.agent}</em>
        {card.description ? `: ${card.description}` : ""}
      </span>
      {card.state === "starting" ? <span> Starting…</span> : null}
      {href ? (
        <Link href={href} className="text-foreground underline underline-offset-4">
          Open<span className="sr-only"> {card.agent}</span>
        </Link>
      ) : null}
      {card.state === "failed" ? (
        <span>
          Did not start{card.reason ? `: ${card.reason}` : ""}
          {onTryAgain ? (
            <>
              {" "}
              <button
                type="button"
                onClick={onTryAgain}
                className="text-foreground underline underline-offset-4"
              >
                Try again
              </button>
            </>
          ) : null}
        </span>
      ) : null}
    </p>
  );
}
