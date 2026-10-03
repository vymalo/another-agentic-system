import { EyeIcon } from "lucide-react";
import { cn } from "@/lib/utils";

/**
 * What stands where the message box would be when the person may read a thread and not act on it
 * (a role without `thread.write`, or one that may not invoke the thread's agent): one line that says
 * so in words (`Read only: your roles do not let you write in threads.`), with an eye for the glance. It
 * is a status, not an error: nothing failed. The words are the state; the colours only back them.
 */
export function ReadOnlyNotice({
  reason,
  isNew = false,
  className,
}: {
  /** The whole sentence, from `threadAccess` (it starts with "Read only" for a thread). */
  reason: string;
  /** The new-chat page: no thread, so no padding for the footer under the box. */
  isNew?: boolean;
  className?: string;
}) {
  return (
    <div className={cn("flex flex-col gap-2", isNew ? "" : "pb-3", className)}>
      <p
        role="status"
        data-slot="read-only"
        className="flex items-center gap-2.5 rounded-3xl border border-input bg-muted px-4 py-3 text-sm text-foreground"
      >
        <EyeIcon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
        <span className="min-w-0 [overflow-wrap:anywhere]">{reason}</span>
      </p>
    </div>
  );
}

/**
 * The header's chip for the same state, so it is seen at the top too: text and an eye, not a colour.
 * A phone's top bar has no room for the word (the agent's name is what gives way): the eye stays and
 * the word is for a screen reader, and the line above the keyboard says it in full.
 */
export function ReadOnlyChip() {
  return (
    <span
      data-slot="read-only-chip"
      className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full bg-muted px-2.5 text-[0.8125rem] text-foreground max-sm:px-2"
    >
      <EyeIcon aria-hidden="true" className="size-3.5 text-muted-foreground" />
      <span className="max-sm:sr-only">Read only</span>
    </span>
  );
}
