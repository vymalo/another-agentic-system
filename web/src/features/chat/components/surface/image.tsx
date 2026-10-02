"use client";

import { ImageOffIcon } from "lucide-react";
import type { ReactNode } from "react";
import { plainName } from "@/features/chat/lib/files";
import { FileImage } from "../cards/kept-file-card";
import { useThreadFiles } from "./thread-files";

/*
 * The `Image` component of the UI catalog (docs/api/ui-catalog-v1.md, version 4, ADR 0032): a
 * picture of a file this thread holds, named by its SHA-256. Never a URL: the file is looked up
 * among the thread's kept files, and what is drawn is an `<img src>` of that file's own `href`
 * (the API's route), so nothing outside the app is fetched and an SVG runs nothing. The validator
 * has refused a surface whose `artifact` is not an image of this thread already; this is the
 * second check (ADR 0013 rule 6), and a file that is not there draws a line, not a broken image.
 * `alt` and `caption` are the agent's: drawn as text.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The Image of a surface; the props are those of the lowered component (`prepare.ts`). */
export function ImageFile(props: Rec): ReactNode {
  const files = useThreadFiles();
  const artifact = str(props.artifact);
  const file = artifact === undefined ? undefined : files.find((f) => f.sha256 === artifact);
  const alt = plainName(str(props.alt) ?? "", 300);
  const caption = str(props.caption);
  if (file?.preview !== "image") {
    return (
      <p
        data-slot="image-missing"
        className="flex items-center gap-2 rounded-md border border-dashed px-2.5 py-2 text-xs text-muted-foreground"
      >
        <ImageOffIcon aria-hidden="true" className="size-4 shrink-0" />
        <span className="[overflow-wrap:anywhere]">
          Image not shown: it is not one of this conversation's files{alt ? ` (${alt})` : ""}.
        </span>
      </p>
    );
  }
  return (
    <figure data-slot="image" className="m-0 flex min-w-0 flex-col gap-1.5">
      <FileImage file={file} alt={alt || undefined} />
      {caption ? (
        <figcaption className="text-xs whitespace-pre-wrap text-muted-foreground [overflow-wrap:anywhere]">
          {caption}
        </figcaption>
      ) : null}
    </figure>
  );
}
