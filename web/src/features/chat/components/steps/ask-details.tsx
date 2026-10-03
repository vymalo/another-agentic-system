"use client";

import type { ReactNode } from "react";
import { safeHttpUrl } from "@/features/chat/lib/a2ui/url";
import type { AskContent } from "@/features/chat/lib/agui/vymalo";
import { ExpandableText } from "../parts/expandable-text";

/** What an ask opens onto: the words are longer than a line's, so the preview is too. */
const ASK_PREVIEW = 600;

function Row({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div data-slot="ask-detail" className="flex min-w-0 flex-col gap-0.5">
      <dt className="text-xs font-medium text-muted-foreground">{title}</dt>
      <dd className="min-w-0 text-xs leading-5 text-foreground/90 [overflow-wrap:anywhere]">
        {children}
      </dd>
    </div>
  );
}

/**
 * What an ask was asked and what it handed back, under its line: the question put to the agent,
 * its last words, the artifacts it names. Every word is an agent's: untrusted text, drawn as text
 * nodes, and a link only when it is an absolute http(s) URL (ADR 0013 rule 6). The question it
 * asked back and the reason it failed are on the line, open or not (`lib/ask.ts`).
 */
export function AskDetails({ id, content }: { id: string; content: AskContent }) {
  return (
    <dl id={id} data-slot="ask-details" className="flex min-w-0 flex-col gap-1.5">
      {content.text ? (
        <Row title="Asked">
          <ExpandableText text={content.text} limit={ASK_PREVIEW} />
        </Row>
      ) : null}
      {content.answer ? (
        <Row title="Answer">
          <ExpandableText text={content.answer} limit={ASK_PREVIEW} />
        </Row>
      ) : null}
      {content.artifacts?.length ? (
        <Row title="Handed back">
          <ul className="flex flex-col gap-0.5">
            {content.artifacts.map((a) => {
              const href = safeHttpUrl(a.uri);
              return (
                <li key={`${a.name}\n${a.uri ?? ""}`} className="min-w-0 truncate">
                  {href ? (
                    <a
                      href={href}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="rounded-sm underline underline-offset-2 hover:text-foreground focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"
                    >
                      {a.name}
                    </a>
                  ) : (
                    a.name
                  )}
                </li>
              );
            })}
          </ul>
        </Row>
      ) : null}
    </dl>
  );
}
