"use client";

import {
  CircleCheckIcon,
  CircleIcon,
  CircleMinusIcon,
  CircleXIcon,
  LoaderCircleIcon,
} from "lucide-react";
import type { ChecklistState } from "@/features/chat/lib/a2ui/status";
import { cn } from "@/lib/utils";

/*
 * The icon of a state (a checklist item, a timeline event). It is decoration: the state is also said in
 * words beside it, so a state is never told by its colour alone.
 */

const ICON = {
  done: CircleCheckIcon,
  doing: LoaderCircleIcon,
  todo: CircleIcon,
  failed: CircleXIcon,
  dropped: CircleMinusIcon,
} as const;

const TONE: Record<ChecklistState, string> = {
  done: "text-success",
  doing: "text-brand",
  todo: "text-muted-foreground",
  failed: "text-destructive",
  dropped: "text-muted-foreground",
};

export function StateIcon({ state, className }: { state: ChecklistState; className?: string }) {
  const Icon = ICON[state];
  return <Icon aria-hidden="true" className={cn("size-4 shrink-0", TONE[state], className)} />;
}
