import { CircleAlertIcon } from "lucide-react";
import type { ReactNode } from "react";
import type { ErrorContent, StatusContent } from "@/features/chat/lib/agui/vymalo";
import { cn } from "@/lib/utils";

/**
 * A soft callout in the flow of the turn: what went wrong and what to do next. Deliberately not
 * `role="alert"`: a replay would announce every old error.
 */
export function Callout({
  title,
  children,
  hint,
  className,
}: {
  title: ReactNode;
  children?: ReactNode;
  hint?: ReactNode;
  className?: string;
}) {
  return (
    <div
      data-slot="error-callout"
      className={cn(
        "flex w-full max-w-xl gap-3 rounded-xl bg-destructive-soft px-4 py-3 text-sm",
        className,
      )}
    >
      <CircleAlertIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-destructive" />
      <div className="flex min-w-0 flex-col gap-0.5">
        <p className="font-medium text-destructive [overflow-wrap:anywhere]">{title}</p>
        {children ? (
          <div className="text-foreground/90 [overflow-wrap:anywhere]">{children}</div>
        ) : null}
        {hint ? <p className="text-[0.8125rem] text-muted-foreground">{hint}</p> : null}
      </div>
    </div>
  );
}

/** An error of the orchestrator (a delivery that failed, a refused part). */
export function ErrorCallout({ data }: { data: ErrorContent }) {
  return (
    <Callout
      title={data.retryable ? "Something went wrong, and it may pass" : "Something went wrong"}
      hint={data.retryable ? "You can send a message to retry." : "Write a message to try again."}
    >
      {data.message}
    </Callout>
  );
}

/** The agent said it failed: its reason, and that the thread goes on with the next message. */
export function FailedCallout({ data }: { data: StatusContent }) {
  const who = data.actor?.type === "agent" ? data.actor.name : "The agent";
  return (
    <Callout
      title={
        <>
          <span className="capitalize">{who}</span> couldn’t finish
        </>
      }
      hint="Write a message to try again or change course."
    >
      {data.detail?.trim() || "It did not say why."}
    </Callout>
  );
}
