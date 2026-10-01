"use client";

import { ExternalLinkIcon } from "lucide-react";
import type { ReactNode } from "react";
import { Badge } from "@/components/ui/badge";
import { type CardSpec, readCards } from "@/features/chat/lib/a2ui/cards";
import { cn } from "@/lib/utils";

/*
 * The `Cards` component of the UI catalog (docs/api/ui-catalog-v1.md, version 3): up to 24 cards in a
 * list or a grid, each a title, an optional subtitle and body, tags and a link. A source list, a set
 * of options, the pages an agent found.
 *
 * What reaches this file has passed the schema of the catalog and `prepareSurface` (which refuses a
 * card whose link is not an absolute http(s) URL); the strings are the agent's, drawn as React text.
 * Nothing is loaded for a card: no image, no favicon, no preview (ADR 0013 rule 5). A link is a plain
 * `<a>` to a URL checked twice, `target="_blank" rel="noopener noreferrer"` (rule 6), and the host it
 * goes to is written next to it, so the person sees where before they follow it. Cards do not send
 * anything: there is no action, and no state.
 */

function CardItem({ card }: { card: CardSpec }) {
  return (
    <li
      data-slot="cards-item"
      className="flex min-w-0 flex-col gap-1.5 rounded-xl border bg-background px-3.5 py-3"
    >
      <div className="flex min-w-0 flex-col gap-0.5">
        <p className="text-sm leading-snug font-medium [overflow-wrap:anywhere]">
          {card.href ? (
            <a
              href={card.href}
              target="_blank"
              rel="noopener noreferrer"
              className="text-brand underline-offset-2 hover:underline focus-visible:rounded-sm focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
            >
              {card.title}
              <span className="sr-only"> (opens in a new tab)</span>
            </a>
          ) : (
            card.title
          )}
        </p>
        {card.subtitle ? (
          <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">{card.subtitle}</p>
        ) : null}
      </div>
      {card.body ? (
        <p className="text-sm whitespace-pre-wrap [overflow-wrap:anywhere]">{card.body}</p>
      ) : null}
      {card.tags.length > 0 || card.host ? (
        <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1.5">
          {card.tags.map((tag, i) => (
            <Badge
              // a tag may repeat: the position is the identity of a chip
              // biome-ignore lint/suspicious/noArrayIndexKey: see above
              key={i}
              variant="secondary"
              className="h-auto max-w-full whitespace-normal [overflow-wrap:anywhere]"
            >
              {tag}
            </Badge>
          ))}
          {card.host ? (
            <span className="ml-auto inline-flex min-w-0 items-center gap-1 text-xs text-muted-foreground">
              <ExternalLinkIcon aria-hidden="true" className="size-3 shrink-0" />
              <span className="[overflow-wrap:anywhere]">{card.host}</span>
            </span>
          ) : null}
        </div>
      ) : null}
    </li>
  );
}

/** The Cards of a surface; the props are those of the lowered component (`prepare.ts`). */
export function CardsList(props: Record<string, unknown>): ReactNode {
  const spec = readCards(props);
  if (!spec) return null;
  return (
    <div data-slot="cards" className="flex min-w-0 flex-col gap-2">
      {spec.title ? (
        // biome-ignore lint/a11y/useSemanticElements: the level is the surface's, not the page's
        <p
          role="heading"
          aria-level={3}
          className="text-base font-semibold [overflow-wrap:anywhere]"
        >
          {spec.title}
        </p>
      ) : null}
      <ul
        // biome-ignore lint/a11y/noRedundantRoles: without a bullet Safari would drop the list role
        role="list"
        data-layout={spec.layout}
        className={cn(
          "m-0 grid min-w-0 list-none grid-cols-1 gap-2 p-0",
          spec.layout === "grid" && "sm:grid-cols-2",
        )}
      >
        {spec.cards.map((card, i) => (
          // the position is the identity of a card: two cards may have the same words
          // biome-ignore lint/suspicious/noArrayIndexKey: see above
          <CardItem key={i} card={card} />
        ))}
      </ul>
    </div>
  );
}
