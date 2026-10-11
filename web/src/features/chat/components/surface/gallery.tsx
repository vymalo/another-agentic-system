"use client";

import { ChevronLeftIcon, ChevronRightIcon, ImageOffIcon } from "lucide-react";
import type { ReactNode } from "react";
import { useLayoutEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { FileImage } from "@/features/chat/components/cards/kept-file-card";
import { type KeptFile, plainName } from "@/features/chat/lib/files";
import { useThreadFiles } from "./thread-files";

/*
 * The `Gallery` component of the UI catalog (docs/api/ui-catalog-v1.md, version 6, group 4): up to twelve pictures of
 * files of this thread, each named by its sha256 and given its alt text. They are a grid; a picture opens larger in a
 * dialog, where the arrow keys, and the buttons, move to the next and the previous one. A picture is drawn the way an
 * `Image` is (`FileImage`, the file's own route), and a file the thread does not hold is a line, never a broken picture.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

type Picture = { alt: string; caption: string; file: KeptFile | undefined };

function readPictures(v: unknown, files: readonly KeptFile[]): Picture[] {
  if (!Array.isArray(v)) return [];
  const out: Picture[] = [];
  for (const item of v) {
    if (typeof item !== "object" || item === null) continue;
    const artifact = str((item as Rec).artifact);
    const file = files.find((f) => f.sha256 === artifact);
    out.push({
      alt: plainName(str((item as Rec).alt) ?? "", 300),
      caption: plainName(str((item as Rec).caption) ?? "", 300),
      // an image only: the validator refuses another kind, and this is the second check
      file: file?.preview === "image" ? file : undefined,
    });
  }
  return out;
}

/** The Gallery of a surface; the props are those of the lowered component (`prepare.ts`). */
export function GalleryView(props: Rec): ReactNode {
  const files = useThreadFiles();
  const pictures = readPictures(props.images, files);
  const title = plainName(str(props.title) ?? "", 120);
  const [open, setOpen] = useState<number | undefined>(undefined);
  const current = open === undefined ? undefined : pictures[open];
  const step = (by: number) =>
    setOpen((i) => (i === undefined ? undefined : (i + by + pictures.length) % pictures.length));
  // the arrows move while a picture is open, whichever element of the dialog has the focus; a layout effect, so
  // the listener is there before the first key of a person who has just seen the picture
  const count = pictures.length;
  useLayoutEffect(() => {
    if (open === undefined) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "ArrowRight" && e.key !== "ArrowLeft") return;
      e.preventDefault();
      const by = e.key === "ArrowRight" ? 1 : -1;
      setOpen((i) => (i === undefined ? undefined : (i + by + count) % count));
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, count]);
  return (
    <div data-slot="gallery" className="flex min-w-0 flex-col gap-2">
      {title ? <p className="text-sm font-medium [overflow-wrap:anywhere]">{title}</p> : null}
      {/* biome-ignore lint/a11y/noRedundantRoles: without a bullet Safari would drop the list role */}
      <ul role="list" className="m-0 grid min-w-0 list-none grid-cols-2 gap-2 p-0 sm:grid-cols-3">
        {pictures.map((picture, i) => (
          <li
            // the pictures are written once and never reordered: the position is the identity of a picture
            // biome-ignore lint/suspicious/noArrayIndexKey: see above
            key={i}
            className="min-w-0"
          >
            {picture.file ? (
              <button
                type="button"
                aria-label={`Open the picture: ${picture.alt}`}
                onClick={() => setOpen(i)}
                data-slot="gallery-item"
                className="block w-full min-w-0 overflow-hidden rounded-lg border bg-muted/30 outline-none focus-visible:ring-3 focus-visible:ring-ring/50"
              >
                <FileImage
                  file={picture.file}
                  alt={picture.alt}
                  className="h-24 w-full object-cover sm:h-32"
                />
              </button>
            ) : (
              <p className="flex h-full items-center gap-2 rounded-md border border-dashed px-2.5 py-2 text-xs text-muted-foreground">
                <ImageOffIcon aria-hidden="true" className="size-4 shrink-0" />
                <span>{`Picture not shown${picture.alt ? `: ${picture.alt}` : ""}.`}</span>
              </p>
            )}
          </li>
        ))}
      </ul>
      <Dialog open={current !== undefined} onOpenChange={(next) => !next && setOpen(undefined)}>
        <DialogContent className="flex max-h-[90vh] min-w-0 flex-col gap-3 sm:max-w-3xl">
          <DialogTitle className="sr-only">{current?.alt || "Picture"}</DialogTitle>
          {current?.file ? (
            <FileImage
              file={current.file}
              alt={current.alt}
              className="max-h-[60vh] w-full object-contain"
            />
          ) : null}
          <div className="flex items-center justify-between gap-2">
            <Button
              type="button"
              variant="outline"
              size="icon-sm"
              aria-label="Previous picture"
              onClick={() => step(-1)}
            >
              <ChevronLeftIcon aria-hidden="true" />
            </Button>
            <p aria-live="polite" className="text-xs text-muted-foreground tabular-nums">
              {open === undefined ? "" : `${open + 1} of ${pictures.length}`}
            </p>
            <Button
              type="button"
              variant="outline"
              size="icon-sm"
              aria-label="Next picture"
              onClick={() => step(1)}
            >
              <ChevronRightIcon aria-hidden="true" />
            </Button>
          </div>
          {current?.caption ? (
            <p className="text-sm [overflow-wrap:anywhere]">{current.caption}</p>
          ) : null}
        </DialogContent>
      </Dialog>
    </div>
  );
}
