"use client";

import {
  GitForkIcon,
  MenuIcon,
  PanelLeftCloseIcon,
  PanelLeftOpenIcon,
  SquarePenIcon,
  XIcon,
} from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { type ReactNode, type Ref, useEffect, useId, useMemo, useState } from "react";
import { PandaMark } from "@/components/brand/panda-mark";
import { InlineStatus, LoadingStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import {
  Sheet,
  SheetClose,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";
import { useShowDescriptions } from "@/features/chat/hooks/use-ui-config";
import { useThreadBranches } from "@/features/threads/components/branches-provider";
import type { ThreadsView } from "@/features/threads/hooks/use-threads";
import { groupByRecency } from "@/features/threads/lib/recency";
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

/**
 * One thread: its title on one line, a link to it. `active` is the open thread. `family` is the
 * thread an open edit was made from, the one the list has in place of the edit (a branch is not
 * a row of its own): it is the open chat for the eye (the same highlight) and `aria-current="true"`
 * for a screen reader, which keeps "page" for the address that is open.
 *
 * A thread with a description (ADR 0035; `description` is undefined when the configuration hides
 * them) shows it in a hover card, which a pointer opens by resting on the row and the keyboard by
 * focusing the link, and a screen reader reads as the link's description. It is plain text: the
 * model wrote it, so nothing here renders it as Markdown.
 */
function ThreadRow({
  thread,
  description,
  active,
  family,
}: {
  thread: ApiThread;
  description?: string | undefined;
  active: boolean;
  family: boolean;
}) {
  const open = active || family;
  const descriptionId = useId();
  const link = (
    <Link
      href={`/threads/${thread.id}`}
      aria-current={active ? "page" : family ? "true" : undefined}
      aria-describedby={description ? descriptionId : undefined}
      className={cn(
        "flex h-10 min-w-0 items-center gap-2 rounded-full px-3 text-sm text-foreground/90 no-underline transition-colors hover:bg-sidebar-accent/70 md:h-9",
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
      <LiveMark state={thread.state} />
    </Link>
  );
  return (
    <li data-slot="thread-row">
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
    </li>
  );
}

/** "New chat", then the threads grouped by recency (newest first), and paging. */
function ThreadNav({ threads }: { threads: ThreadsView }) {
  const pathname = usePathname();
  // the open thread may be an edit, which the list leaves out: its conversation's first thread is the row
  const { root } = useThreadBranches();
  const showDescriptions = useShowDescriptions();
  // the groups are relative to today; computed on the client, after the first render
  const [now, setNow] = useState<Date | null>(null);
  useEffect(() => setNow(new Date()), []);
  const groups = useMemo(
    () =>
      now
        ? groupByRecency(threads.threads, now)
        : [{ group: "Recent" as const, items: threads.threads }],
    [threads.threads, now],
  );
  return (
    <nav aria-label="Threads" className="flex flex-col gap-1 px-2 pb-4">
      <Link
        href="/"
        className="mb-3 flex h-10 items-center gap-2.5 rounded-full px-3 text-sm font-medium text-foreground no-underline transition-colors hover:bg-sidebar-accent/70"
      >
        <SquarePenIcon aria-hidden="true" className="size-4 text-muted-foreground" />
        New chat
      </Link>
      {threads.error ? (
        <div className="px-1">
          <InlineStatus tone="error" action={{ label: "Retry", onClick: threads.refresh }}>
            Could not load threads.
          </InlineStatus>
        </div>
      ) : threads.loading ? (
        <LoadingStatus className="px-1 pt-1">Loading threads…</LoadingStatus>
      ) : threads.threads.length === 0 ? (
        <p className="px-3 text-sm text-muted-foreground">Your chats will show up here.</p>
      ) : null}
      {groups.map(({ group, items }) => (
        <div key={group} className="flex flex-col pb-3">
          <h2 className="px-3 pb-1 text-xs font-medium text-muted-foreground">{group}</h2>
          <ul className="flex flex-col gap-px">
            {items.map((t) => (
              <ThreadRow
                key={t.id}
                thread={t}
                description={showDescriptions ? t.description : undefined}
                active={pathname === `/threads/${t.id}`}
                family={t.id === root}
              />
            ))}
          </ul>
        </div>
      ))}
      {threads.hasMore ? (
        <Button
          type="button"
          variant="ghost"
          className="h-9 justify-start rounded-full px-3 text-muted-foreground"
          onClick={threads.loadMore}
        >
          Load older
        </Button>
      ) : null}
    </nav>
  );
}

/** The brand row of the sidebar, with the control that collapses it (a desktop). */
function SidebarTop({
  onCollapse,
  collapseRef,
  close,
}: {
  onCollapse?: () => void;
  collapseRef?: Ref<HTMLButtonElement>;
  close?: ReactNode;
}) {
  return (
    <div className="flex h-14 shrink-0 items-center justify-between ps-4 pe-2">
      <Link
        href="/"
        className="flex items-center gap-2 rounded-full text-[0.9375rem] font-semibold tracking-tight text-foreground no-underline"
      >
        <PandaMark size={28} />
        <span>
          another<span className="text-brand">·</span>agentic
        </span>
      </Link>
      {onCollapse ? (
        <Button
          type="button"
          variant="ghost"
          size="icon"
          ref={collapseRef}
          aria-label="Close sidebar"
          title="Close sidebar"
          onClick={onCollapse}
          // colours only: `transition-all` would also transition the `visibility` this button
          // inherits from the sidebar, and the focus moved here on opening could land on its first
          // frame, still hidden
          className="size-9 rounded-full text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground"
        >
          <PanelLeftCloseIcon aria-hidden="true" className="size-4.5" />
        </Button>
      ) : null}
      {close}
    </div>
  );
}

/**
 * Wide screens: a column beside the chat that the person can collapse (the shell remembers it).
 * Hidden on phones, where `ThreadsSheet` opens the same list.
 */
export function ThreadSidebar({
  threads,
  open,
  onCollapse,
  collapseRef,
}: {
  threads: ThreadsView;
  open: boolean;
  onCollapse: () => void;
  /** The "Close sidebar" button, which takes the focus when the sidebar opens. */
  collapseRef?: Ref<HTMLButtonElement>;
}) {
  return (
    <aside
      data-slot="sidebar"
      data-state={open ? "open" : "closed"}
      className={cn(
        "hidden min-h-0 shrink-0 overflow-hidden bg-sidebar transition-[width] duration-200 ease-out md:block dark:border-r",
        open ? "w-68" : "w-0 border-r-0",
      )}
    >
      <div
        className={cn("flex h-full w-68 flex-col", !open && "invisible")}
        aria-hidden={open ? undefined : true}
      >
        <SidebarTop onCollapse={onCollapse} collapseRef={collapseRef} />
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
          <ThreadNav threads={threads} />
        </div>
      </div>
    </aside>
  );
}

/** A desktop with the sidebar collapsed: the controls to open it again and to start a chat. */
export function SidebarOpeners({
  onOpen,
  openRef,
}: {
  onOpen: () => void;
  /** The "Open sidebar" button, which takes the focus when the sidebar closes. */
  openRef?: Ref<HTMLButtonElement>;
}) {
  return (
    <div className="hidden items-center gap-0.5 md:flex">
      <Button
        type="button"
        variant="ghost"
        size="icon"
        ref={openRef}
        aria-label="Open sidebar"
        title="Open sidebar"
        onClick={onOpen}
        className="size-9 rounded-full text-muted-foreground hover:text-foreground"
      >
        <PanelLeftOpenIcon aria-hidden="true" className="size-4.5" />
      </Button>
      <Button
        asChild
        variant="ghost"
        size="icon"
        className="size-9 rounded-full text-muted-foreground hover:text-foreground"
      >
        <Link href="/" aria-label="New chat" title="New chat">
          <SquarePenIcon aria-hidden="true" className="size-4.5" />
        </Link>
      </Button>
    </div>
  );
}

/**
 * Phones: a menu button ("Threads") that opens the list in a sheet from the left. The sheet
 * content is unmounted while closed, so only one "Threads" navigation exists at a time, and it
 * closes when the route changes (a thread was picked). Focus returns to the button on close.
 */
export function ThreadsSheet({ threads }: { threads: ThreadsView }) {
  const [open, setOpen] = useState(false);
  const pathname = usePathname();
  // biome-ignore lint/correctness/useExhaustiveDependencies: closing on navigation is the point
  useEffect(() => setOpen(false), [pathname]);

  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          aria-label="Threads"
          className="size-10 rounded-full text-muted-foreground hover:text-foreground md:hidden"
        >
          <MenuIcon aria-hidden="true" className="size-5" />
        </Button>
      </SheetTrigger>
      <SheetContent
        side="left"
        showCloseButton={false}
        aria-describedby={undefined}
        className="w-[min(18rem,85vw)] gap-0 border-r-0 bg-sidebar p-0 shadow-xl data-[side=left]:w-[min(18rem,85vw)]"
      >
        <SheetHeader className="sr-only">
          <SheetTitle>Threads</SheetTitle>
        </SheetHeader>
        <SidebarTop
          close={
            <SheetClose asChild>
              <Button
                type="button"
                variant="ghost"
                size="icon"
                aria-label="Close"
                className="size-9 rounded-full text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
              >
                <XIcon aria-hidden="true" className="size-4.5" />
              </Button>
            </SheetClose>
          }
        />
        <div className="min-h-0 flex-1 overflow-y-auto overscroll-contain">
          <ThreadNav threads={threads} />
        </div>
      </SheetContent>
    </Sheet>
  );
}
