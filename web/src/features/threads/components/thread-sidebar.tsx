"use client";

import { usePathname } from "next/navigation";
import { useEffect, useState } from "react";
import {
  ThreadListItems,
  ThreadListNew,
  ThreadListRoot,
} from "@/components/assistant-ui/elements/thread-list.aui";
import { InlineStatus, LoadingStatus } from "@/components/inline-status";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Sheet, SheetContent, SheetHeader, SheetTitle, SheetTrigger } from "@/components/ui/sheet";
import type { ThreadsView } from "@/features/threads/hooks/use-threads";

/** The thread list: "New thread", the threads (newest first), and paging. */
function ThreadNav({ threads }: { threads: ThreadsView }) {
  return (
    <nav aria-label="Threads" className="p-3">
      <ThreadListRoot>
        <ThreadListNew />
        {threads.error ? (
          <InlineStatus tone="error" action={{ label: "Retry", onClick: threads.refresh }}>
            Could not load threads.
          </InlineStatus>
        ) : threads.loading ? (
          <LoadingStatus className="pt-1">Loading threads…</LoadingStatus>
        ) : threads.threads.length === 0 ? (
          <InlineStatus>No threads yet.</InlineStatus>
        ) : null}
        <ThreadListItems />
        {threads.hasMore ? (
          <Button
            type="button"
            variant="link"
            className="h-11 justify-start px-2.5 md:h-9"
            onClick={threads.loadMore}
          >
            Load older
          </Button>
        ) : null}
      </ThreadListRoot>
    </nav>
  );
}

/** Wide screens: a fixed column. Hidden on phones, where `ThreadsSheet` opens the same list. */
export function ThreadSidebar({ threads }: { threads: ThreadsView }) {
  return (
    <aside className="hidden min-h-0 border-r bg-muted md:block">
      <ScrollArea className="h-full">
        <ThreadNav threads={threads} />
      </ScrollArea>
    </aside>
  );
}

/**
 * Phones: a "Threads" button that opens the list in a sheet from the left. The sheet content is
 * unmounted while closed, so only one "Threads" navigation exists at a time, and it closes when
 * the route changes (a thread was picked). Focus returns to the button on close.
 */
export function ThreadsSheet({ threads }: { threads: ThreadsView }) {
  const [open, setOpen] = useState(false);
  const pathname = usePathname();
  // biome-ignore lint/correctness/useExhaustiveDependencies: closing on navigation is the point
  useEffect(() => setOpen(false), [pathname]);

  return (
    <Sheet open={open} onOpenChange={setOpen}>
      <SheetTrigger asChild>
        <Button type="button" variant="outline" className="h-11">
          Threads
        </Button>
      </SheetTrigger>
      <SheetContent
        side="left"
        aria-describedby={undefined}
        className="w-72 gap-0 shadow-none data-[side=left]:w-72"
      >
        <SheetHeader className="border-b">
          <SheetTitle className="text-base">Threads</SheetTitle>
        </SheetHeader>
        <ScrollArea className="min-h-0 flex-1">
          <ThreadNav threads={threads} />
        </ScrollArea>
      </SheetContent>
    </Sheet>
  );
}
