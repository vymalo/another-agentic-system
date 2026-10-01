import type { ReactNode } from "react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";

const ROWS = ["a", "b", "c", "d", "e"];

type Action = { label: string; onClick: () => void };

function ActionButton({ action, className }: { action: Action; className?: string }) {
  return (
    <Button
      type="button"
      variant="link"
      size="sm"
      className={cn("h-auto px-0 py-1", className)}
      onClick={action.onClick}
    >
      {action.label}
    </Button>
  );
}

/**
 * Empty, loading and error states are a quiet inline line, never a hero. An error is a
 * destructive `Alert` (a live region unless `role` says otherwise). A warning is a line on the
 * warning colours: something is missing or degraded, and the rest works (a source of agents that
 * could not be read).
 */
export function InlineStatus({
  children,
  tone = "muted",
  action,
  role,
}: {
  children: ReactNode;
  tone?: "muted" | "warning" | "error";
  action?: Action;
  role?: "status" | "alert";
}) {
  if (tone === "error") {
    return (
      <Alert variant="destructive" {...(role ? { role } : {})}>
        <AlertDescription className="flex flex-wrap items-baseline gap-x-2 text-destructive">
          <span>{children}</span>
          {action ? <ActionButton action={action} /> : null}
        </AlertDescription>
      </Alert>
    );
  }
  if (tone === "warning") {
    return (
      <p
        className="my-1 flex flex-wrap items-baseline gap-x-2 rounded-md bg-warning-soft px-3 py-2 text-sm text-warning"
        role={role}
      >
        <span>{children}</span>
        {action ? <ActionButton action={action} className="text-warning" /> : null}
      </p>
    );
  }
  return (
    <p
      className="my-1 flex flex-wrap items-baseline gap-x-2 text-sm text-muted-foreground"
      role={role}
    >
      <span>{children}</span>
      {action ? <ActionButton action={action} /> : null}
    </p>
  );
}

/**
 * Skeleton rows for content that is loading. The text is for screen readers (and tests); the
 * rows are decoration.
 */
export function LoadingStatus({
  children,
  rows = 3,
  className,
}: {
  children: string;
  rows?: number;
  className?: string;
}) {
  return (
    <div role="status" className={cn("flex flex-col gap-2", className)}>
      <span className="sr-only">{children}</span>
      {ROWS.slice(0, rows).map((row) => (
        <Skeleton key={row} aria-hidden="true" className="h-8 w-full" />
      ))}
    </div>
  );
}
