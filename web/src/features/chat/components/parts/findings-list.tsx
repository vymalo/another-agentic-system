"use client";

import { useState } from "react";
import { Button } from "@/components/ui/button";
import { keyed, pluralFindings } from "@/features/chat/lib/findings";
import { ExpandableText, MORE } from "./expandable-text";

/** A finding longer than this is cut, with a control to read the rest. */
export const FINDING_PREVIEW = 240;
/** More findings than this are folded behind a control. */
export const FINDINGS_SHOWN = 5;

function Finding({ text }: { text: string }) {
  return (
    <li data-slot="finding" className="[overflow-wrap:anywhere]">
      <ExpandableText text={text} limit={FINDING_PREVIEW} />
    </li>
  );
}

/**
 * What a source found wrong, as plain text. Long findings are cut with an expand control, and a
 * long list is folded after {@link FINDINGS_SHOWN}.
 */
export function FindingsList({
  findings,
  heading = true,
}: {
  findings: readonly string[];
  /** "Findings (n)" over the list; off where a disclosure's summary says it already. */
  heading?: boolean;
}) {
  const [all, setAll] = useState(false);
  if (findings.length === 0) return null;
  const folded = !all && findings.length > FINDINGS_SHOWN;
  const shown = keyed(folded ? findings.slice(0, FINDINGS_SHOWN) : findings);
  return (
    <div data-slot="findings" className="flex flex-col gap-1">
      {heading ? (
        <p className="text-xs font-medium text-muted-foreground">Findings ({findings.length})</p>
      ) : null}
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
