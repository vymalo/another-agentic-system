"use client";

import { BotIcon, ExternalLinkIcon } from "lucide-react";
import Link from "next/link";
import { useThreadView } from "@/features/chat/components/thread-view";
import { cn } from "@/lib/utils";
import { parseSession } from "../lib/cards";
import { isHomeCard, sessionStateWords, threadHref } from "../lib/words";

/**
 * The card of a subagent (a helper session the agent opened, child-sessions/v1) in the parent's
 * transcript: who it is, what it was started for, where it stands and a link to its own thread.
 *
 * The same card is said again in several runs; it is drawn only in its home run (`home`, which
 * `ThreadAgent` sets), with the state the stream said last. Everything the agent wrote (the name, the
 * description) is text: nothing here is Markdown. The link is made of a thread id that is a UUID.
 */
export function SessionCard({ data }: { data: unknown }) {
  const { sessions } = useThreadView();
  const own = isHomeCard(data) ? parseSession(data) : null;
  if (!own) return null;
  const id = (data as { card?: unknown }).card;
  const card = (typeof id === "string" ? sessions?.get(id) : undefined) ?? own;
  const name = card.agent || "subagent";
  const href = threadHref(card.threadId);
  const words = sessionStateWords(card);
  const active = card.state === "working" && card.report !== "blocked";
  return (
    <article
      data-slot="session-card"
      data-state={card.state}
      aria-label={`Subagent ${name}`}
      className="flex w-full max-w-xl min-w-0 flex-col gap-1.5 rounded-xl border bg-card p-3.5"
    >
      <div className="flex min-w-0 items-center gap-2.5">
        <span
          aria-hidden="true"
          className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground"
        >
          <BotIcon className="size-4" />
        </span>
        <div className="min-w-0 flex-1">
          <p
            data-slot="session-name"
            className="truncate text-sm font-medium [overflow-wrap:anywhere]"
          >
            {name}
          </p>
          {card.description ? (
            <p
              data-slot="session-description"
              className="text-[0.8125rem] text-muted-foreground [overflow-wrap:anywhere]"
            >
              {card.description}
            </p>
          ) : null}
        </div>
        {href ? (
          <Link
            href={href}
            className="inline-flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-sm text-foreground underline-offset-4 hover:underline focus-visible:outline-2 focus-visible:outline-ring"
          >
            Open
            <span className="sr-only"> {name}</span>
            <ExternalLinkIcon aria-hidden="true" className="size-3.5" />
          </Link>
        ) : null}
      </div>
      <p
        data-slot="session-status"
        className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground"
      >
        <span className="inline-flex items-center gap-1.5">
          <span
            aria-hidden="true"
            className={cn(
              "size-1.5 rounded-full bg-muted-foreground/60",
              active && "bg-brand motion-safe:animate-pulse",
              card.report === "blocked" && "bg-warning",
            )}
          />
          <span data-slot="session-state">{words}</span>
        </span>
        {card.round !== undefined ? <span>Round {card.round}</span> : null}
        {card.by === "person" ? <span>Started by you</span> : null}
      </p>
    </article>
  );
}
