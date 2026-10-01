"use client";

import { DownloadIcon, EllipsisIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ThreadExporter } from "@/features/chat/hooks/use-export-thread";

/**
 * The thread's overflow menu. "Export JSON" downloads the whole thread as a file to send to a
 * developer; it is disabled until the thread is known and while a download is being fetched.
 */
export function ThreadMenu({
  exporter,
  disabled,
}: {
  exporter: ThreadExporter;
  disabled: boolean;
}) {
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
      <DropdownMenuContent align="end">
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
