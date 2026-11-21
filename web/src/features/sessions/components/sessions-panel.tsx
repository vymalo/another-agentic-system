"use client";

import { BotIcon, ExternalLinkIcon, Loader2Icon, PlusIcon } from "lucide-react";
import Link from "next/link";
import { useId } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import type { SessionCard, SessionStartCard } from "../lib/cards";
import { subagentStateWords, threadHref } from "../lib/words";
import { useSessions } from "./sessions-provider";

/** The Subagents tab's label count: how many are open. */
export function subagentsCount(open: number): string | undefined {
  return open > 0 ? String(open) : undefined;
}

/**
 * The subagents of the open thread, in the right-hand panel (child-sessions/v1, web/DESIGN.md "Panel"):
 * the open ones first, each with its name, what it is for, where it stands in words, whether it stays
 * open or closes when done, a link to its own thread and Close; the closed ones folded apart; and
 * "New subagent" at the end, disabled with its reason in view while the agent's limit is reached. The
 * helper's name and the description are the agent's words, drawn as text.
 */
export function SessionsPanel() {
  const sessions = useSessions();
  const reasonId = useId();
  if (!sessions) return null;
  const { open, closed, pending } = sessions.children;
  const empty = open.length + closed.length + pending.length === 0;
  const blocked = sessions.newBlocked;
  return (
    <div data-slot="sessions-panel" className="flex min-w-0 flex-col gap-3 p-4">
      {empty ? (
        <p className="text-sm text-muted-foreground">
          No subagents yet.
          {sessions.canStart
            ? " Start one to work on part of this while you keep talking here."
            : ""}
        </p>
      ) : (
        <>
          <ul aria-label="Subagents" className="flex flex-col gap-2">
            {pending.map((start) => (
              <PendingRow key={start.request} start={start} />
            ))}
            {open.map((card) => (
              <Row key={card.session} card={card} />
            ))}
          </ul>
          {closed.length > 0 ? (
            <details data-slot="sessions-closed" className="group/closed">
              <summary className="cursor-pointer rounded-md py-1 text-sm text-muted-foreground select-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring">
                {closed.length} closed
              </summary>
              <ul aria-label="Closed subagents" className="mt-2 flex flex-col gap-2">
                {closed.map((card) => (
                  <Row key={card.session} card={card} />
                ))}
              </ul>
            </details>
          ) : null}
        </>
      )}
      {sessions.canStart ? (
        <div className="flex flex-col items-start gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={blocked !== null}
            aria-describedby={blocked ? reasonId : undefined}
            onClick={(e) => sessions.openNew(e.currentTarget)}
          >
            <PlusIcon aria-hidden="true" />
            New subagent
          </Button>
          {blocked ? (
            <p
              id={reasonId}
              data-slot="new-subagent-reason"
              className="text-xs text-muted-foreground"
            >
              {blocked.reason}
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function Mark({ children }: { children: string }) {
  return (
    <span className="rounded-full bg-muted px-2 py-0.5 text-[0.6875rem] text-muted-foreground">
      {children}
    </span>
  );
}

function Row({ card }: { card: SessionCard }) {
  const sessions = useSessions();
  const name = card.agent || "subagent";
  const href = threadHref(card.threadId);
  const words = subagentStateWords(card);
  const closing = card.state !== "closed" && (card.closing || card.closeAsked);
  const active = card.state === "working" && card.report !== "blocked" && !closing;
  const offered = sessions?.canWrite === true && sessions.closeOffered(card);
  return (
    <li
      data-slot="subagent-row"
      data-state={card.state}
      className="flex min-w-0 flex-col gap-1.5 rounded-xl border bg-card p-3"
    >
      <div className="flex min-w-0 items-start gap-2.5">
        <BotIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <p data-slot="subagent-name" className="text-sm font-medium [overflow-wrap:anywhere]">
            {name}
          </p>
          {card.description ? (
            <p
              data-slot="subagent-description"
              className="text-[0.8125rem] text-muted-foreground [overflow-wrap:anywhere]"
            >
              {card.description}
            </p>
          ) : null}
        </div>
      </div>
      <p className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
        <span className="inline-flex items-center gap-1.5">
          {closing ? (
            <Loader2Icon aria-hidden="true" className="size-3 motion-safe:animate-spin" />
          ) : (
            <span
              aria-hidden="true"
              className={cn(
                "size-1.5 rounded-full bg-muted-foreground/60",
                active && "bg-brand motion-safe:animate-pulse",
                card.report === "blocked" && card.state !== "closed" && "bg-warning",
              )}
            />
          )}
          <span data-slot="subagent-state">{words}</span>
        </span>
        <Mark>{card.keepOpen ? "Stays open" : "Closes when done"}</Mark>
      </p>
      {href || offered ? (
        <div className="flex items-center gap-1">
          {href ? (
            <Button asChild variant="ghost" size="sm" className="-ms-2">
              <Link href={href}>
                Open <span className="sr-only">{name}</span>
                <ExternalLinkIcon aria-hidden="true" />
              </Link>
            </Button>
          ) : null}
          {offered ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={(e) => sessions.askClose(card, e.currentTarget)}
            >
              Close <span className="sr-only">{name}</span>
            </Button>
          ) : null}
        </div>
      ) : null}
    </li>
  );
}

function PendingRow({ start }: { start: SessionStartCard }) {
  return (
    <li
      data-slot="subagent-row"
      data-state="starting"
      className="flex min-w-0 flex-col gap-1.5 rounded-xl border bg-card p-3"
    >
      <div className="flex min-w-0 items-start gap-2.5">
        <BotIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <p data-slot="subagent-name" className="text-sm font-medium [overflow-wrap:anywhere]">
            {start.agent}
          </p>
          <p
            data-slot="subagent-description"
            className="text-[0.8125rem] text-muted-foreground [overflow-wrap:anywhere]"
          >
            {start.description}
          </p>
        </div>
      </div>
      <p className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
        <span className="inline-flex items-center gap-1.5">
          <Loader2Icon aria-hidden="true" className="size-3 motion-safe:animate-spin" />
          <span data-slot="subagent-state">Starting…</span>
        </span>
        <Mark>{start.keepOpen ? "Stays open" : "Closes when done"}</Mark>
      </p>
    </li>
  );
}
