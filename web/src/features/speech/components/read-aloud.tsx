"use client";

import { SquareIcon, Volume2Icon } from "lucide-react";
import { useEffect, useMemo } from "react";
import { TooltipIconButton } from "@/components/assistant-ui/elements/tooltip-icon-button";
import { cn } from "@/lib/utils";
import { stopSpeechIf, useSpeech } from "../hooks/use-speech";
import { speechText } from "../lib/speech-text";

type Props = {
  /** What tells this answer from the others on the page: the run it came from, or an id of its own. */
  id: string;
  /** The answer's Markdown source (what Copy takes), read as the words the page shows. */
  text: string;
  className?: string;
};

/**
 * Read aloud, under an agent's answer (ADR 0067 S1): the device's own voice, no network and nothing stored. It is **not
 * drawn at all** where the browser has no `speechSynthesis` (the store says so), nor for a turn with no words to say —
 * the same rule as Copy, applied to what is *spoken*: a turn that is only a surface (`[Interface: …]`, a placeholder for
 * Copy) or only a rule draws no button.
 *
 * One answer at a time, app-wide: while this one is read the button is **pressed** (`aria-pressed`), the icon is a
 * square and the tooltip says "Stop reading" — its name stays "Read aloud", as Copy keeps "Copy" and shows "Copied".
 * Starting another answer stops this one, and a read stops when the turn leaves the page. A browser that refuses (for
 * example `not-allowed`) is said in words in a polite status line beside the button, on the answer it is about, as
 * Copy's refusal is.
 */
export function ReadAloud({ id, text, className }: Props) {
  const { supported, speaking, failure, toggle } = useSpeech();
  // what the voice says, worked out when the answer changes: an answer can be long, and most are never read
  const words = useMemo(() => speechText(text), [text]);
  // the answer leaves the page (a thread change, a turn replaced): its read stops with it
  useEffect(() => () => stopSpeechIf(id), [id]);
  if (!supported || words === "") return null;
  const reading = speaking === id;
  const refused = failure?.id === id ? failure.message : "";
  return (
    <>
      <TooltipIconButton
        tooltip={reading ? "Stop reading" : "Read aloud"}
        aria-label="Read aloud"
        aria-pressed={reading}
        data-slot="read-aloud"
        side="bottom"
        className={cn("size-7 rounded-md p-0 [&_svg]:size-3.5", className)}
        onClick={() => toggle(id, words)}
      >
        {reading ? <SquareIcon aria-hidden="true" /> : <Volume2Icon aria-hidden="true" />}
      </TooltipIconButton>
      <span
        aria-live="polite"
        data-slot="speech-note"
        className={refused ? "ms-1 text-xs text-muted-foreground" : "sr-only"}
      >
        {refused}
      </span>
    </>
  );
}
