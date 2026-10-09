import { type LucideIcon, PauseIcon } from "lucide-react";
import { STATE_SHAPE } from "@/features/chat/components/state-shapes";
import type { SummaryIcon, TurnState } from "@/features/chat/lib/step-tree";
import { cn } from "@/lib/utils";

const GLYPH: Record<SummaryIcon, { icon: LucideIcon; tone: string; spin?: boolean }> = {
  spinner: { icon: STATE_SHAPE.working, tone: "text-brand", spin: true },
  check: { icon: STATE_SHAPE.done, tone: "text-muted-foreground" },
  cross: { icon: STATE_SHAPE.failed, tone: "text-destructive" },
  pause: { icon: PauseIcon, tone: "text-warning" },
  verifying: { icon: STATE_SHAPE.verifying, tone: "text-verifying" },
  stopped: { icon: STATE_SHAPE.stopped, tone: "text-muted-foreground" },
};

/** The glyph of a turn's state: a spinner only while it runs, the rest say it by their shape. */
export function turnGlyph(state: TurnState): SummaryIcon {
  switch (state) {
    case "running":
      return "spinner";
    case "verifying":
      return "verifying";
    case "waiting":
      return "pause";
    case "failed":
      return "cross";
    case "canceled":
      return "stopped";
    default:
      return "check";
  }
}

/** A turn's state as a small glyph; decorative, the words beside it say the same. */
export function TurnGlyph({ icon, className }: { icon: SummaryIcon; className?: string }) {
  const { icon: Icon, tone, spin } = GLYPH[icon];
  return (
    <Icon
      aria-hidden="true"
      data-glyph={icon}
      strokeWidth={2.25}
      className={cn("size-4 shrink-0", tone, spin && "motion-safe:animate-spin", className)}
    />
  );
}
