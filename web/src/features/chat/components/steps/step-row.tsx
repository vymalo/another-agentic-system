import { CheckIcon, LoaderCircleIcon, type LucideIcon, PauseIcon, XIcon } from "lucide-react";
import { type ComponentProps, createContext, type ReactNode, useContext } from "react";
import { cn } from "@/lib/utils";

/**
 * How a step stands. `done` and `failed` are finished; `live` is the step the agent is on (the
 * last one of a running turn); `pending` waits for an answer (a check); `waiting` is paused on a
 * question to the person; `warning` sent the agent back; `muted` decided nothing (a stale answer,
 * a stopped run).
 */
export type StepState = "done" | "live" | "failed" | "pending" | "waiting" | "warning" | "muted";

const RING: Record<StepState, string> = {
  done: "bg-muted text-muted-foreground",
  live: "bg-brand/10 text-brand",
  failed: "bg-destructive-soft text-destructive",
  pending: "bg-verifying/10 text-verifying",
  waiting: "bg-warning-soft text-warning",
  warning: "bg-warning-soft text-warning",
  muted: "bg-muted text-muted-foreground",
};

/** The icon of a step: its own glyph once finished, a spinner while it runs or waits. */
function StepIcon({ state, icon: Icon }: { state: StepState; icon: LucideIcon | undefined }) {
  const Glyph =
    state === "live" || state === "pending"
      ? LoaderCircleIcon
      : state === "waiting"
        ? PauseIcon
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

/**
 * What a list that positions its rows itself (the panel's virtualised level) needs to reach the
 * `<li>` of a step: its ref, style and class, and its place in the list. It is handed down by
 * context to the next `StepRow` below, so the renderers of each kind of step need not forward it;
 * a row takes it and clears it for what it holds (a step's own children are not in that list).
 */
export type RowProps = Pick<ComponentProps<"li">, "ref" | "style" | "className"> & {
  "data-index"?: number;
  "aria-setsize"?: number;
  "aria-posinset"?: number;
};

export const RowPropsContext = createContext<RowProps | null>(null);

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
 * the step under them. The lists are `steps/steps-pane.tsx` and `steps/step-node.tsx`.
 */
export function StepRow({ state, icon, label, children, className, ...li }: Props) {
  const placed = useContext(RowPropsContext);
  return (
    <li
      data-slot="step"
      data-state={state}
      {...li}
      {...placed}
      className={cn(
        "group/step relative flex gap-3 pb-2.5 last:pb-0",
        className,
        placed?.className,
      )}
    >
      {/* the rail: a hairline from this icon to the next one */}
      <span
        aria-hidden="true"
        className="absolute top-5 bottom-0 left-2.5 w-px -translate-x-1/2 bg-border group-last/step:hidden"
      />
      <RowPropsContext.Provider value={null}>
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
      </RowPropsContext.Provider>
    </li>
  );
}
