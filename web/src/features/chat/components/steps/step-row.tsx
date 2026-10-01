import { CheckIcon, LoaderCircleIcon, type LucideIcon, XIcon } from "lucide-react";
import type { ComponentProps, ReactNode } from "react";
import { cn } from "@/lib/utils";

/**
 * How a step stands. `done` and `failed` are finished; `live` is the step the agent is on (the
 * last one of a running turn); `pending` waits for an answer (a check); `warning` sent the agent
 * back; `muted` decided nothing (a stale answer, a stopped run).
 */
export type StepState = "done" | "live" | "failed" | "pending" | "warning" | "muted";

const RING: Record<StepState, string> = {
  done: "bg-muted text-muted-foreground",
  live: "bg-primary/10 text-primary",
  failed: "bg-destructive-soft text-destructive",
  pending: "bg-verifying/10 text-verifying",
  warning: "bg-warning-soft text-warning",
  muted: "bg-muted text-muted-foreground",
};

/** The icon of a step: its own glyph once finished, a spinner while it runs or waits. */
function StepIcon({ state, icon: Icon }: { state: StepState; icon: LucideIcon | undefined }) {
  const Glyph =
    state === "live" || state === "pending"
      ? LoaderCircleIcon
      : state === "failed"
        ? (Icon ?? XIcon)
        : (Icon ?? CheckIcon);
  return (
    <span
      aria-hidden="true"
      className={cn(
        "relative z-10 flex size-5 items-center justify-center rounded-full ring-4 ring-background",
        RING[state],
      )}
    >
      <Glyph
        className={cn(
          "size-3",
          (state === "live" || state === "pending") && "motion-safe:animate-spin",
        )}
        strokeWidth={2.25}
      />
    </span>
  );
}

type Props = Omit<ComponentProps<"li">, "children"> & {
  state: StepState;
  /** The step's own glyph (a branch, a terminal); a check or a cross when it has none. */
  icon?: LucideIcon;
  /** The one line that says what happened. */
  label: ReactNode;
  /** Anything under the line: a command, a summary, findings, a link. */
  children?: ReactNode;
};

/**
 * One line of an agent's step list: the icon on a hairline rail, the words, and what belongs to
 * the step under them. The list is `steps/step-list.tsx`.
 */
export function StepRow({ state, icon, label, children, className, ...li }: Props) {
  return (
    <li
      data-slot="step"
      data-state={state}
      className={cn("group/step relative flex gap-3 pb-2.5 last:pb-0", className)}
      {...li}
    >
      {/* the rail: a hairline from this icon to the next one */}
      <span
        aria-hidden="true"
        className="absolute top-5 bottom-0 left-2.5 w-px -translate-x-1/2 bg-border group-last/step:hidden"
      />
      <StepIcon state={state} icon={icon} />
      <div className="flex min-w-0 flex-1 flex-col gap-1.5 pt-px">
        <div
          className={cn(
            "flex min-h-5 min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[0.8125rem] leading-5",
            state === "failed" ? "text-foreground" : "text-foreground/85",
            state === "muted" && "text-muted-foreground",
          )}
        >
          {label}
        </div>
        {children}
      </div>
    </li>
  );
}
