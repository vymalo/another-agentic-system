"use client";

import {
  MenuIcon,
  PanelLeftCloseIcon,
  PanelLeftOpenIcon,
  SquarePenIcon,
  XIcon,
} from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { type ReactNode, type Ref, useEffect, useMemo, useState } from "react";
import { BrandMark } from "@/components/brand-mark";
import { InlineStatus, LoadingStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import {
  Sheet,
  SheetClose,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@/components/ui/sheet";
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
          className="size-1.5 rounded-full bg-primary motion-safe:animate-pulse"
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

/** One thread: its title on one line, a link to it. */
function ThreadRow({ thread, active }: { thread: ApiThread; active: boolean }) {
  return (
    <li data-slot="thread-row">
      <Link
        href={`/threads/${thread.id}`}
        aria-current={active ? "page" : undefined}
        className={cn(
          "flex h-10 min-w-0 items-center gap-2 rounded-full px-3 text-sm text-foreground/90 no-underline transition-colors hover:bg-sidebar-accent/70 md:h-9",
          active && "bg-sidebar-accent font-medium text-foreground",
        )}
      >
        <span className="min-w-0 flex-1 truncate">{thread.title || "Untitled"}</span>
        <LiveMark state={thread.state} />
      </Link>
    </li>
  );
}

/** "New chat", then the threads grouped by recency (newest first), and paging. */
function ThreadNav({ threads }: { threads: ThreadsView }) {
  const pathname = usePathname();
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
              <ThreadRow key={t.id} thread={t} active={pathname === `/threads/${t.id}`} />
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
        <BrandMark />
        <span>
          another<span className="text-muted-foreground">·</span>agentic
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
          className="size-9 rounded-full text-muted-foreground hover:bg-sidebar-accent hover:text-foreground"
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
