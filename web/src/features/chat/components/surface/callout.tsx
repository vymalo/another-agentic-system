"use client";

import { CircleCheckIcon, CircleXIcon, InfoIcon, TriangleAlertIcon } from "lucide-react";
import type { ReactNode } from "react";
import { plainName } from "@/features/chat/lib/files";
import { cn } from "@/lib/utils";

/*
 * The `Callout` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6): a note, a success, a warning or
 * an error, with a title and a text. The tone is said in a word above the title and shown by an icon, so it is
 * never told by colour alone. Output only: nothing is sent, and a replayed answer says the same again without
 * being announced.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

const TONE = {
  info: {
    Icon: InfoIcon,
    word: "Note",
    box: "border-border bg-card",
    icon: "text-muted-foreground",
  },
  success: {
    Icon: CircleCheckIcon,
    word: "Success",
    box: "border-success/40 bg-success/10",
    icon: "text-success",
  },
  warning: {
    Icon: TriangleAlertIcon,
    word: "Warning",
    box: "border-warning/50 bg-warning-soft",
    icon: "text-warning",
  },
  error: {
    Icon: CircleXIcon,
    word: "Error",
    box: "border-destructive/40 bg-destructive-soft",
    icon: "text-destructive",
  },
} as const;

type Tone = keyof typeof TONE;
const isTone = (v: unknown): v is Tone => typeof v === "string" && Object.hasOwn(TONE, v);

/** The Callout of a surface; the props are those of the lowered component (`prepare.ts`). */
export function CalloutView(props: Rec): ReactNode {
  const tone: Tone = isTone(props.tone) ? props.tone : "info";
  const { Icon, word, box, icon } = TONE[tone];
  const title = plainName(str(props.title) ?? "", 120);
  const text = str(props.text) ?? "";
  return (
    <div
      data-slot="callout"
      data-tone={tone}
      className={cn("flex min-w-0 gap-2.5 rounded-xl border px-3.5 py-3", box)}
    >
      <Icon aria-hidden="true" className={cn("mt-0.5 size-4 shrink-0", icon)} />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <p className="text-xs font-medium text-muted-foreground">{word}</p>
        <p className="text-sm font-medium [overflow-wrap:anywhere]">{title}</p>
        <p className="text-sm whitespace-pre-wrap [overflow-wrap:anywhere]">{text}</p>
      </div>
    </div>
  );
}
