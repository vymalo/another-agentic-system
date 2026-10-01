"use client";

import { DownloadIcon, EllipsisIcon, PencilIcon } from "lucide-react";
import { useRef } from "react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ThreadExporter } from "@/features/chat/hooks/use-export-thread";
import type { ThreadRenamer } from "@/features/chat/hooks/use-rename-thread";

/**
 * The thread's overflow menu. "Rename" turns the title in the header into a field. "Export JSON"
 * downloads the whole thread as a file to send to a developer; both are disabled until the
 * thread is known, and the export while a download is being fetched.
 */
export function ThreadMenu({
  exporter,
  renamer,
  disabled,
}: {
  exporter: ThreadExporter;
  renamer: ThreadRenamer;
  disabled: boolean;
}) {
  /** The menu closes and would hand the focus back to its button: the title field has it instead. */
  const renaming = useRef(false);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
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
          if (!renaming.current) return;
          renaming.current = false;
          event.preventDefault();
          renamer.field.current?.focus();
          renamer.field.current?.select();
        }}
      >
        <DropdownMenuItem
          disabled={disabled || renamer.saving}
          onSelect={() => {
            renaming.current = true;
            renamer.start();
          }}
          title="Give this chat another title"
        >
          <PencilIcon aria-hidden="true" />
          Rename
        </DropdownMenuItem>
        <DropdownMenuItem
          disabled={disabled || exporter.exporting}
          onSelect={exporter.run}
          title="Download this chat as a JSON file, to share with a developer"
        >
          <DownloadIcon aria-hidden="true" />
          {exporter.exporting ? "Exporting…" : "Export JSON"}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
