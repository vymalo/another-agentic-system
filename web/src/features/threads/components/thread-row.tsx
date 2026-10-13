"use client";

import {
  ArchiveIcon,
  ArchiveRestoreIcon,
  CornerLeftUpIcon,
  EllipsisVerticalIcon,
  GitForkIcon,
  PinIcon,
  PinOffIcon,
  Trash2Icon,
} from "lucide-react";
import Link from "next/link";
import { type ReactNode, useId, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { ShareMark } from "@/features/sharing/components/share-chip";
import type { ArrangeAction } from "@/features/threads/hooks/use-threads";
import { actionsFor, type RowAction, type Section } from "@/features/threads/lib/rail";
import type { ApiThread } from "@/lib/api/types";
import { cn } from "@/lib/utils";

/** A small mark after a thread that needs nothing from you (working) or waits for you. */
function LiveMark({ state }: { state: ApiThread["state"] }) {
  if (state === "queued" || state === "working" || state === "verifying") {
    return (
      <span className="flex shrink-0 items-center">
        <span
          aria-hidden="true"
          className="size-1.5 rounded-full bg-brand motion-safe:animate-pulse"
        />
        <span className="sr-only"> (working)</span>
      </span>
    );
  }
  if (state === "blocked") {
    return (
      <span className="flex shrink-0 items-center">
        <span aria-hidden="true" className="size-1.5 rounded-full bg-warning" />
        <span className="sr-only"> (waiting for you)</span>
      </span>
    );
  }
  return null;
}

const ICONS: Record<RowAction, typeof PinIcon> = {
  pin: PinIcon,
  unpin: PinOffIcon,
  archive: ArchiveIcon,
  unarchive: ArchiveRestoreIcon,
  eject: CornerLeftUpIcon,
  delete: Trash2Icon,
};

/** What the person does to a row: an arrangement of the list, or the delete that has a dialog. */
export type RowHandlers = {
  onArrange: (thread: ApiThread, action: ArrangeAction) => void;
  /** Delete was chosen; `from` is the row's menu button, where the focus goes back if the chat stays. */
  onDelete: (thread: ApiThread, from: HTMLElement | null) => void;
};

/**
 * The row's menu (ADR 0042, decisions 4 and 6; ADR 0043): a vertical three-dot button (no tooltip: the focus
 * that comes back to it after a move would open one over the next row's button) that shows when
 * the row is hovered or the keyboard is on it (always on a touch screen, where there is no hover), and
 * what the row's kind offers (`actionsFor`). It is a control of its own beside the title's link, never inside
 * it. Choosing Delete leaves the focus to the dialog that follows; the menu would put it back on this button.
 */
function RowMenu({
  thread,
  section,
  mayDelete,
  onArrange,
  onDelete,
}: { thread: ApiThread; section: Section; mayDelete: boolean } & RowHandlers) {
  const choices = actionsFor(thread, { section, mayDelete });
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement | null>(null);
  const deleting = useRef(false);
  if (choices.length === 0) return null;
  const plain = choices.filter((c) => c.kind !== "delete");
  const erase = choices.find((c) => c.kind === "delete");
  const choose = (kind: RowAction) => {
    if (kind === "delete") {
      deleting.current = true;
      onDelete(thread, trigger.current);
    } else onArrange(thread, kind);
  };
  const item = (c: (typeof choices)[number]) => {
    const Icon = ICONS[c.kind];
    return (
      <DropdownMenuItem
        key={c.kind}
        title={c.hint}
        onSelect={() => choose(c.kind)}
        className={
          c.kind === "delete"
            ? "text-destructive focus:text-destructive [&_svg]:text-destructive"
            : undefined
        }
      >
        <Icon aria-hidden="true" />
        {c.label}
      </DropdownMenuItem>
    );
  };
  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Button
          ref={trigger}
          type="button"
          variant="ghost"
          size="icon"
          data-slot="thread-menu-trigger"
          aria-label={`More actions for ${thread.title || "Untitled"}`}
          className={cn(
            "absolute inset-y-0 end-1 my-auto size-7 rounded-full text-muted-foreground opacity-0",
            "hover:bg-sidebar-accent hover:text-foreground focus-visible:opacity-100",
            "group-hover/row:opacity-100 group-has-[:focus-visible]/row:opacity-100",
            "data-[state=open]:bg-sidebar-accent data-[state=open]:opacity-100",
            "pointer-coarse:size-9 pointer-coarse:opacity-100",
          )}
        >
          <EllipsisVerticalIcon aria-hidden="true" className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        className="w-52"
        onCloseAutoFocus={(event) => {
          // the delete dialog has the focus now, and gives it back when it closes
          if (!deleting.current) return;
          deleting.current = false;
          event.preventDefault();
        }}
      >
        {plain.map(item)}
        {erase ? (
          <>
            {plain.length > 0 ? <DropdownMenuSeparator /> : null}
            {item(erase)}
          </>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/**
 * One thread: its title on one line, a link to it, and its menu. `active` is the open thread. `family`
 * is the thread an open edit was made from, the one the list has in place of the edit (a branch is not
 * a row of its own): it is the open chat for the eye (the same highlight) and `aria-current="true"`
 * for a screen reader, which keeps "page" for the address that is open. `nested` is the list of the
 * threads under it (ADR 0042: one level), drawn in the item of its parent.
 *
 * A thread with a description (ADR 0035; `description` is undefined when the configuration hides
 * them) shows it in a hover card, which a pointer opens by resting on the row and the keyboard by
 * focusing the link, and a screen reader reads as the link's description. It is plain text: the
 * model wrote it, so nothing here renders it as Markdown.
 */
export function ThreadRow({
  thread,
  description,
  active,
  family,
  section,
  mayDelete,
  nested,
  onArrange,
  onDelete,
}: {
  thread: ApiThread;
  description?: string | undefined;
  active: boolean;
  family: boolean;
  section: Section;
  mayDelete: boolean;
  nested?: ReactNode;
} & RowHandlers) {
  const open = active || family;
  const descriptionId = useId();
  const link = (
    <Link
      href={`/threads/${thread.id}`}
      aria-current={active ? "page" : family ? "true" : undefined}
      aria-describedby={description ? descriptionId : undefined}
      className={cn(
        "flex min-w-0 items-center gap-2 rounded-full px-3 text-sm text-foreground/90 no-underline transition-colors",
        "group-hover/row:bg-sidebar-accent/70 group-has-data-[state=open]/row:bg-sidebar-accent/70",
        // room for the menu button, which covers the end of the row while it shows
        "group-hover/row:pe-9 group-has-[:focus-visible]/row:pe-9 group-has-data-[state=open]/row:pe-9 pointer-coarse:pe-11",
        "h-10 md:h-9",
        open && "bg-sidebar-accent font-medium text-foreground",
      )}
    >
      {thread.forkedFrom?.kind === "fork" ? (
        // a conversation made from another (ADR 0029): "Fork from here", or another agent's
        <GitForkIcon aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
      ) : null}
      <span className="min-w-0 flex-1 truncate">
        {thread.title || "Untitled"}
        {thread.forkedFrom?.kind === "fork" ? <span className="sr-only"> (fork)</span> : null}
      </span>
      {thread.share ? <ShareMark share={thread.share} /> : null}
      <LiveMark state={thread.state} />
    </Link>
  );
  return (
    <li data-slot="thread-item">
      <div data-slot="thread-row" data-thread-id={thread.id} className="group/row relative">
        {description ? (
          <>
            <HoverCard openDelay={400} closeDelay={100}>
              <HoverCardTrigger asChild>{link}</HoverCardTrigger>
              <HoverCardContent side="right" aria-hidden="true" data-slot="thread-description-card">
                <p className="text-sm font-medium break-words text-foreground">
                  {thread.title || "Untitled"}
                </p>
                <p className="mt-1 text-[0.8125rem] break-words text-muted-foreground">
                  {description}
                </p>
              </HoverCardContent>
            </HoverCard>
            <span id={descriptionId} className="sr-only">
              {description}
            </span>
          </>
        ) : (
          link
        )}
        <RowMenu
          thread={thread}
          section={section}
          mayDelete={mayDelete}
          onArrange={onArrange}
          onDelete={onDelete}
        />
      </div>
      {nested}
    </li>
  );
}
