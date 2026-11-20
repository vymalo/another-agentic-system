"use client";

import { BotIcon } from "lucide-react";
import Link from "next/link";
import { useThreadView } from "@/features/chat/components/thread-view";
import { parseSessionStart } from "../lib/cards";
import { isHomeCard, threadHref } from "../lib/words";
import { useSessions } from "./sessions-provider";

/**
 * The muted line of a start a person asked for ("You started planner: plan the release"), above the
 * turn, in the voice of the person's other acts (`ToolsLine`): Starting…, then Open, or why it did not
 * start. It is said again in the run that answers it; it is drawn only in its home run (`home`), with
 * the state the stream said last. The agent's words (the name, the description, the reason) are text.
 * "Try again" is offered while the person's draft is still kept (the contract keeps no text): it reopens the
 * "New subagent" dialog with it. `onTryAgain` is for a caller that has its own way; it is given the button.
 */
export function StartLine({
  data,
  onTryAgain,
}: {
  data: unknown;
  onTryAgain?: (opener: HTMLElement) => void;
}) {
  const { sessionStarts } = useThreadView();
  const sessions = useSessions();
  const own = isHomeCard(data) ? parseSessionStart(data) : null;
  if (!own) return null;
  const card = sessionStarts?.get(own.request) ?? own;
  const href = card.state === "opened" ? threadHref(card.threadId) : undefined;
  const again =
    onTryAgain ??
    (sessions?.canRetry(card.request)
      ? (opener: HTMLElement) => sessions.retry(card.request, opener)
      : undefined);
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
          {again ? (
            <>
              {" "}
              <button
                type="button"
                onClick={(e) => again(e.currentTarget)}
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
