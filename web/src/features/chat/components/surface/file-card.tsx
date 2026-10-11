"use client";

import { FileIcon, ImageOffIcon } from "lucide-react";
import type { ReactNode } from "react";
import { DownloadControl } from "@/features/chat/components/cards/kept-file-card";
import { mustFetch } from "@/features/chat/lib/file-access";
import { fileTitle, formatSize } from "@/features/chat/lib/files";
import { useBrowserAuth } from "@/lib/auth/use-browser-auth";
import { useThreadFiles } from "./thread-files";

/*
 * The `FileCard` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6): a file the thread holds,
 * named by its SHA-256, drawn as its name, its size and type, and a download. The file is looked up among the
 * thread's kept files (the same list an `Image` reads), and its download is the same control the file cards
 * of the conversation use (`DownloadControl`, through `lib/file-access.ts`): nothing outside the app is
 * fetched. A file the thread does not hold is a line that says so, never a card.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The FileCard of a surface; the props are those of the lowered component (`prepare.ts`). */
export function FileCardView(props: Rec): ReactNode {
  const files = useThreadFiles();
  const auth = useBrowserAuth();
  const artifact = str(props.artifact);
  const file = artifact === undefined ? undefined : files.find((f) => f.sha256 === artifact);
  if (!file) {
    return (
      <p
        data-slot="file-card-missing"
        className="flex items-center gap-2 rounded-md border border-dashed px-2.5 py-2 text-xs text-muted-foreground"
      >
        <ImageOffIcon aria-hidden="true" className="size-4 shrink-0" />
        <span>This file is not one of this conversation's files.</span>
      </p>
    );
  }
  const title = fileTitle(file);
  const details = [formatSize(file.size), file.mimeType].filter(Boolean).join(" · ");
  return (
    <div
      data-slot="file-card"
      className="flex w-full max-w-xl min-w-0 items-center gap-3 rounded-xl border bg-card p-3.5"
    >
      <span
        aria-hidden="true"
        className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground"
      >
        <FileIcon className="size-4.5" />
      </span>
      <div className="min-w-0 flex-1">
        <p data-slot="file-name" className="text-sm font-medium [overflow-wrap:anywhere]">
          {title}
        </p>
        {details ? (
          <p
            data-slot="file-details"
            className="text-xs text-muted-foreground [overflow-wrap:anywhere]"
          >
            {details}
          </p>
        ) : null}
      </div>
      <DownloadControl file={file} title={title} fetched={mustFetch(auth, file.href)} />
    </div>
  );
}
