"use client";

import { ExternalLinkIcon } from "lucide-react";
import type { ReactNode } from "react";
import { hostOf } from "@/features/chat/lib/a2ui/cards";
import { safeHttpUrl } from "@/features/chat/lib/a2ui/url";
import { plainName } from "@/features/chat/lib/files";

/*
 * The `LinkPreview` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6, group 4): a link with the title,
 * the description and the site the agent gave it, and the host the link goes to. Nothing is fetched for it: no page is
 * read, no image, no favicon, no status; a preview the agent did not write is not made here. The link is an http(s) URL
 * checked twice (`safeHttpUrl`), opened in a new tab; a URL that does not pass is drawn as text.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The LinkPreview of a surface; the props are those of the lowered component (`prepare.ts`). */
export function LinkPreviewView(props: Rec): ReactNode {
  const href = safeHttpUrl(props.url);
  const title = plainName(str(props.title) ?? "", 200);
  const description = str(props.description) ?? "";
  const site = plainName(str(props.site) ?? "", 100);
  const host = href ? hostOf(href) : undefined;
  return (
    <article
      data-slot="link-preview"
      className="flex min-w-0 flex-col gap-1 rounded-xl border bg-card px-3.5 py-3"
    >
      {site ? (
        <p className="text-xs text-muted-foreground [overflow-wrap:anywhere]">{site}</p>
      ) : null}
      <p className="text-sm leading-snug font-medium [overflow-wrap:anywhere]">
        {href ? (
          <a
            href={href}
            target="_blank"
            rel="noopener noreferrer"
            className="text-brand underline-offset-2 hover:underline focus-visible:rounded-sm focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
          >
            {title}
            <span className="sr-only"> (opens in a new tab)</span>
          </a>
        ) : (
          title
        )}
      </p>
      {description ? (
        <p className="text-sm whitespace-pre-wrap [overflow-wrap:anywhere]">{description}</p>
      ) : null}
      {host ? (
        <p className="inline-flex min-w-0 items-center gap-1 text-xs text-muted-foreground">
          <ExternalLinkIcon aria-hidden="true" className="size-3 shrink-0" />
          <span className="[overflow-wrap:anywhere]">{host}</span>
        </p>
      ) : null}
    </article>
  );
}
