"use client";

import type { ReactNode } from "react";
import { plainName } from "@/features/chat/lib/files";
import { CopyButton } from "./copy-button";

/*
 * The `Code` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6): a block of source code
 * in a monospace box, with its title and file name, and a copy button. No highlighting (no highlighter is a
 * dependency of the web), no link, nothing run: the code is React text. The box scrolls sideways and is
 * reachable by keyboard, so a long line is never lost.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The Code of a surface; the props are those of the lowered component (`prepare.ts`). */
export function CodeBlock(props: Rec): ReactNode {
  const code = str(props.code) ?? "";
  const title = plainName(str(props.title) ?? "", 120);
  const filename = plainName(str(props.filename) ?? "", 200);
  const language = plainName(str(props.language) ?? "", 40);
  const name = title || filename || "Code";
  return (
    <figure
      data-slot="code"
      data-language={language || undefined}
      className="m-0 flex min-w-0 flex-col overflow-hidden rounded-xl border bg-muted/40"
    >
      <figcaption className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 border-b px-3 py-2 text-xs text-muted-foreground">
        {title ? (
          <span className="min-w-0 text-sm font-medium text-foreground [overflow-wrap:anywhere]">
            {title}
          </span>
        ) : null}
        {filename ? (
          <span className="min-w-0 font-mono [overflow-wrap:anywhere]">{filename}</span>
        ) : null}
        {language ? <span>{language}</span> : null}
        <span className="ml-auto">
          <CopyButton text={code} label="Copy code" />
        </span>
      </figcaption>
      {/* biome-ignore lint/a11y/useSemanticElements: a scrollable box, not a section of the page */}
      <pre
        // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must be reachable by keyboard
        tabIndex={0}
        role="region"
        aria-label={name}
        className="m-0 max-h-96 overflow-auto p-3 font-mono text-xs leading-relaxed whitespace-pre"
      >
        <code>{code}</code>
      </pre>
    </figure>
  );
}
