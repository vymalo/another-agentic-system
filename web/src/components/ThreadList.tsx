import { ThreadListItemPrimitive, ThreadListPrimitive } from "@assistant-ui/react";
import { useEffect, useState } from "react";
import type { ThreadsView } from "@/chat/use-threads";
import { InlineStatus } from "./InlineStatus";

const NARROW = "(max-width: 719px)";

export function ThreadList({ threads }: { threads: ThreadsView }) {
  // Wide screens: always visible. Phones: a "Threads" disclosure, collapsed on load.
  const [open, setOpen] = useState(true);
  useEffect(() => {
    if (window.matchMedia(NARROW).matches) setOpen(false);
  }, []);

  return (
    <details
      className="sidebar"
      open={open}
      onToggle={(e) => setOpen((e.currentTarget as HTMLDetailsElement).open)}
    >
      <summary className="sidebar__summary">Threads</summary>
      <nav aria-label="Threads" className="sidebar__nav">
        <ThreadListPrimitive.Root className="threads">
          <ThreadListPrimitive.New className="btn btn--secondary threads__new">
            New thread
          </ThreadListPrimitive.New>
          {threads.error ? (
            <InlineStatus tone="error" action={{ label: "Retry", onClick: threads.refresh }}>
              Could not load threads.
            </InlineStatus>
          ) : threads.loading ? (
            <InlineStatus role="status">Loading threads…</InlineStatus>
          ) : threads.threads.length === 0 ? (
            <InlineStatus>No threads yet.</InlineStatus>
          ) : null}
          <ThreadListPrimitive.Items>
            {() => (
              <ThreadListItemPrimitive.Root className="threads__item">
                <ThreadListItemPrimitive.Trigger className="threads__trigger">
                  <ThreadListItemPrimitive.Title fallback="Untitled" />
                </ThreadListItemPrimitive.Trigger>
              </ThreadListItemPrimitive.Root>
            )}
          </ThreadListPrimitive.Items>
          {threads.hasMore ? (
            <button type="button" className="link-button" onClick={threads.loadMore}>
              Load older
            </button>
          ) : null}
        </ThreadListPrimitive.Root>
      </nav>
    </details>
  );
}
