import { CircleAlertIcon } from "lucide-react";
import type { ReactNode } from "react";
import type { ErrorContent, StatusContent } from "@/features/chat/lib/agui/vymalo";
import { splitFailure } from "@/features/chat/lib/failure-text";
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

/**
 * What went wrong, as untrusted text: the first line is the message, and what follows (a type checker's code
 * frames, a stack, a long finding) is behind a "Show details" disclosure, preformatted, so a long failure never
 * fills the screen. Native `<details>`: the summary is a button the keyboard reaches, and nothing here parses
 * markup (React escapes the text).
 */
export function FailureText({ text }: { text: string }) {
  const { message, details } = splitFailure(text);
  return (
    <>
      <span data-slot="failure-message">{message}</span>
      {details ? (
        <details data-slot="failure-details" className="group mt-1.5">
          <summary
            className={cn(
              "w-fit cursor-pointer list-none rounded-sm text-xs text-muted-foreground underline underline-offset-2",
              "hover:text-foreground focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50",
              "[&::-webkit-details-marker]:hidden",
            )}
          >
            <span className="group-open:hidden">Show details</span>
            <span className="hidden group-open:inline">Hide details</span>
          </summary>
          {/* A region the keyboard can scroll: a scrollable area must take focus (WCAG 2.1.1, axe
              `scrollable-region-focusable`), and a named region is what a screen reader reads it as. */}
          <section
            // biome-ignore lint/a11y/noNoninteractiveTabindex: the focus is how the keyboard scrolls the block
            tabIndex={0}
            aria-label="Failure details"
            data-slot="failure-scroll"
            className={cn(
              "mt-1.5 max-h-64 overflow-auto rounded-md bg-background/60 p-2",
              "focus-visible:outline-none focus-visible:ring-3 focus-visible:ring-ring/50",
            )}
          >
            <pre
              data-slot="failure-pre"
              className="font-mono text-xs leading-relaxed whitespace-pre-wrap [overflow-wrap:normal] [word-break:normal]"
            >
              {details}
            </pre>
          </section>
        </details>
      ) : null}
    </>
  );
}

/** An error of the orchestrator (a delivery that failed, a refused part). */
export function ErrorCallout({ data }: { data: ErrorContent }) {
  return (
    <Callout
      title={data.retryable ? "Something went wrong, and it may pass" : "Something went wrong"}
      hint={data.retryable ? "You can send a message to retry." : "Write a message to try again."}
    >
      <FailureText text={data.message} />
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
      <FailureText text={data.detail?.trim() || "It did not say why."} />
    </Callout>
  );
}
