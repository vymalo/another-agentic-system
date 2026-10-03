"use client";

import {
  DownloadIcon,
  EllipsisIcon,
  PencilIcon,
  Share2Icon,
  TextCursorInputIcon,
} from "lucide-react";
import { useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ThreadDescriber } from "@/features/chat/hooks/use-describe-thread";
import type { ThreadExporter } from "@/features/chat/hooks/use-export-thread";
import type { ThreadRenamer } from "@/features/chat/hooks/use-rename-thread";
import { ShareDialog } from "@/features/sharing/components/share-dialog";
import type { ThreadSharer } from "@/features/sharing/hooks/use-share-thread";
import type { Sharing } from "@/lib/api/types";

/**
 * The thread's overflow menu. "Rename" turns the title in the header into a field, and "Edit
 * description" (or "Add description") the line under it (not offered when the configuration hides
 * descriptions: no `describer`). "Export JSON" downloads the whole thread as a file to send to a
 * developer. All are disabled until the thread is known, and the export while a download is being
 * fetched. On a thread the person may only read, rename and describe are disabled, with the reason as
 * their title; the export is a read and stays. "Share…" (ADR 0040) opens the share dialog, and is there
 * only when the page passes a `share` (the deployment lets the person share, or the thread is shared:
 * taking a link down needs only ownership).
 */
export function ThreadMenu({
  exporter,
  renamer,
  describer,
  hasDescription = false,
  disabled,
  readOnly,
  share,
}: {
  exporter: ThreadExporter;
  renamer: ThreadRenamer;
  describer?: ThreadDescriber;
  /** The thread has a description: the item says "Edit", else "Add". */
  hasDescription?: boolean;
  disabled: boolean;
  /** The thread is the person's to read and not to change: why (`Read only: …`). Rename and describe are off. */
  readOnly?: string;
  /** Sharing the thread: who it is shared with and what the person may share as. Absent: no "Share…". */
  share?: { sharer: ThreadSharer; cap: Sharing };
}) {
  /** The menu closes and would hand the focus back to its button: the field being edited has it instead. */
  const editing = useRef<ThreadRenamer | null>(null);
  const trigger = useRef<HTMLButtonElement | null>(null);
  const [sharing, setSharing] = useState(false);
  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            ref={trigger}
            type="button"
            variant="ghost"
            size="icon"
            aria-label="Thread options"
            className="size-9 rounded-full text-muted-foreground hover:text-foreground"
          >
            <EllipsisIcon aria-hidden="true" className="size-5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent
          align="end"
          onCloseAutoFocus={(event) => {
            const editor = editing.current;
            if (!editor) return;
            editing.current = null;
            event.preventDefault();
            editor.field.current?.focus();
            editor.field.current?.select();
          }}
        >
          <DropdownMenuItem
            disabled={disabled || renamer.saving || readOnly !== undefined}
            onSelect={() => {
              editing.current = renamer;
              renamer.start();
            }}
            title={readOnly ?? "Give this chat another title"}
          >
            <PencilIcon aria-hidden="true" />
            Rename
          </DropdownMenuItem>
          {describer ? (
            <DropdownMenuItem
              disabled={disabled || describer.saving || readOnly !== undefined}
              onSelect={() => {
                editing.current = describer;
                describer.start();
              }}
              title={
                readOnly ??
                "Say what this chat is about. What you write stays: the model does not change it."
              }
            >
              <TextCursorInputIcon aria-hidden="true" />
              {hasDescription ? "Edit description" : "Add description"}
            </DropdownMenuItem>
          ) : null}
          <DropdownMenuItem
            disabled={disabled || exporter.exporting}
            onSelect={exporter.run}
            title="Download this chat as a JSON file, to share with a developer"
          >
            <DownloadIcon aria-hidden="true" />
            {exporter.exporting ? "Exporting…" : "Export JSON"}
          </DropdownMenuItem>
          {share ? (
            <DropdownMenuItem
              disabled={disabled}
              onSelect={() => setSharing(true)}
              title="Let people read this chat with a link"
            >
              <Share2Icon aria-hidden="true" />
              Share…
            </DropdownMenuItem>
          ) : null}
        </DropdownMenuContent>
      </DropdownMenu>
      {share ? (
        <ShareDialog
          open={sharing}
          onOpenChange={setSharing}
          sharer={share.sharer}
          cap={share.cap}
          // the menu that opened it is gone: the focus goes back to its button, not to the page
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            trigger.current?.focus();
          }}
        />
      ) : null}
    </>
  );
}
