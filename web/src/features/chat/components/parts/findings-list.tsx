"use client";

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { keyed, pluralFindings, truncate } from "@/features/chat/lib/findings";

/** A finding longer than this is cut, with a control to read the rest. */
export const FINDING_PREVIEW = 240;
/** More findings than this are folded behind a control. */
export const FINDINGS_SHOWN = 5;

const MORE =
  "h-auto cursor-pointer p-0 text-xs underline underline-offset-2 hover:underline focus-visible:ring-3";

function Finding({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const preview = truncate(text, FINDING_PREVIEW);
  return (
    <li data-slot="finding" className="[overflow-wrap:anywhere]">
      {/* plain text in a text node: React escapes it, and nothing here parses markup */}
      <span className="whitespace-pre-wrap">{open ? text : preview.text}</span>
      {preview.cut ? (
        <>
          {" "}
          <Button
            type="button"
            variant="link"
            size="xs"
            className={MORE}
            aria-expanded={open}
            onClick={() => setOpen((o) => !o)}
          >
            {open ? "Show less" : "Show more"}
          </Button>
        </>
      ) : null}
    </li>
  );
}

/**
 * What a source found wrong, as plain text. Long findings are cut with an expand control, and a
 * long list is folded after {@link FINDINGS_SHOWN}.
 */
export function FindingsList({ findings }: { findings: readonly string[] }) {
  const [all, setAll] = useState(false);
  if (findings.length === 0) return null;
  const folded = !all && findings.length > FINDINGS_SHOWN;
  const shown = keyed(folded ? findings.slice(0, FINDINGS_SHOWN) : findings);
  return (
    <div data-slot="findings" className="flex flex-col gap-1">
      <p className="text-xs font-medium text-muted-foreground">Findings ({findings.length})</p>
      <ul className="list-disc space-y-1 pl-5">
        {shown.map((f) => (
          <Finding key={f.key} text={f.text} />
        ))}
      </ul>
      {findings.length > FINDINGS_SHOWN ? (
        <Button
          type="button"
          variant="link"
          size="xs"
          className={`${MORE} self-start`}
          aria-expanded={all}
          onClick={() => setAll((a) => !a)}
        >
          {all ? `Show fewer findings` : `Show all ${pluralFindings(findings.length)}`}
        </Button>
      ) : null}
    </div>
  );
}
