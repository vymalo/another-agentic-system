"use client";

import { ThreadListItemPrimitive, ThreadListPrimitive } from "@assistant-ui/react";
import { PlusIcon } from "lucide-react";
import { type ComponentPropsWithoutRef, type FC, forwardRef } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/*
 * Pruned from the assistant-ui `thread-list` registry item. What the event log has no handlers for
 * is gone: archive, delete, rename and the "more" menu, search, date groups and the running spinner.
 */

export const ThreadListRoot: FC<ComponentPropsWithoutRef<typeof ThreadListPrimitive.Root>> = ({
  className,
  ...props
}) => (
  <ThreadListPrimitive.Root
    data-slot="aui_thread-list-root"
    className={cn("flex flex-col gap-1", className)}
    {...props}
  />
);

export const ThreadListNew = forwardRef<HTMLButtonElement, ComponentPropsWithoutRef<typeof Button>>(
  ({ className, children, ...props }, ref) => (
    <ThreadListPrimitive.New asChild>
      <Button
        ref={ref}
        variant="outline"
        data-slot="aui_thread-list-new"
        className={cn("h-11 justify-start gap-2 px-2.5 md:h-9", className)}
        {...props}
      >
        {children ?? (
          <>
            <PlusIcon aria-hidden="true" className="size-4 shrink-0" />
            New thread
          </>
        )}
      </Button>
    </ThreadListPrimitive.New>
  ),
);

ThreadListNew.displayName = "ThreadListNew";

/** The threads, one `<li>` per thread (newest first, the order the runtime gets them in). */
export const ThreadListItems: FC<ComponentPropsWithoutRef<"ul">> = ({ className, ...props }) => (
  <ul
    data-slot="aui_thread-list-items"
    className={cn("flex flex-col gap-0.5", className)}
    {...props}
  >
    <ThreadListPrimitive.Items>{() => <ThreadListItem />}</ThreadListPrimitive.Items>
  </ul>
);

export const ThreadListItem: FC = () => (
  <ThreadListItemPrimitive.Root asChild>
    <li
      data-slot="aui_thread-list-item"
      className="group relative rounded-md transition-colors hover:bg-background data-active:bg-background data-active:ring-1 data-active:ring-border"
    >
      <ThreadListItemPrimitive.Trigger
        data-slot="aui_thread-list-item-trigger"
        className="flex min-h-11 w-full min-w-0 cursor-pointer items-center rounded-md px-2.5 py-1.5 text-start text-sm outline-none focus-visible:ring-3 focus-visible:ring-ring/50 group-data-active:font-semibold md:min-h-9"
      >
        <span className="min-w-0 flex-1 truncate">
          <ThreadListItemPrimitive.Title fallback="Untitled" />
        </span>
      </ThreadListItemPrimitive.Trigger>
    </li>
  </ThreadListItemPrimitive.Root>
);
