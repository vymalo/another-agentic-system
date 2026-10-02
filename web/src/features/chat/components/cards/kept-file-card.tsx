"use client";

import { DownloadIcon, FileIcon, FileTextIcon, ImageIcon, ImageOffIcon } from "lucide-react";
import { useEffect, useState } from "react";
import {
  downloadHref,
  fileAlt,
  fileTitle,
  formatSize,
  type KeptFile,
  readTextPreview,
  TEXT_PREVIEW_BYTES,
  type TextPreview,
} from "@/features/chat/lib/files";

/*
 * A file the agent made and the artifact store kept (ADR 0032): its name, its size, a download, and
 * a preview when the type has one. Everything about the file is the agent's text and drawn as text.
 * The bytes are fetched from the file's `href` and from nothing else: an image is an `<img src>`
 * (never inline markup, so an SVG runs nothing and loads nothing), a text file is read (the first
 * 64 KiB) and drawn in a `<pre>`, and any other type is the card alone.
 */

/** A picture of the file; if the browser cannot decode it, the card says so and keeps the download. */
export function FileImage({
  file,
  alt,
  className,
}: {
  file: KeptFile;
  /** What the picture says in words; the file's name when the agent gave no better. */
  alt?: string | undefined;
  className?: string;
}) {
  const [broken, setBroken] = useState(false);
  if (broken) {
    return (
      <p
        data-slot="file-image-error"
        className="flex items-center gap-2 rounded-md border border-dashed px-2.5 py-2 text-xs text-muted-foreground"
      >
        <ImageOffIcon aria-hidden="true" className="size-4 shrink-0" />
        <span>The image could not be shown. You can still download it.</span>
      </p>
    );
  }
  return (
    // biome-ignore lint/performance/noImgElement: a same-origin file the API serves; next/image would proxy it
    <img
      data-slot="file-image"
      src={file.href}
      alt={alt ?? fileAlt(file)}
      loading="lazy"
      decoding="async"
      referrerPolicy="no-referrer"
      onError={() => setBroken(true)}
      className={className ?? "max-h-96 max-w-full rounded-lg border bg-muted/30 object-contain"}
    />
  );
}

type Loaded = { state: "loading" } | { state: "error" } | ({ state: "ready" } & TextPreview);

/** The first bytes of a text file, as text in a scrollable box. */
function FileText({ file }: { file: KeptFile }) {
  const [loaded, setLoaded] = useState<Loaded>({ state: "loading" });
  useEffect(() => {
    const controller = new AbortController();
    setLoaded({ state: "loading" });
    readTextPreview(file.href, fetch, controller.signal).then(
      (preview) => setLoaded({ state: "ready", ...preview }),
      () => {
        if (!controller.signal.aborted) setLoaded({ state: "error" });
      },
    );
    return () => controller.abort();
  }, [file.href]);

  if (loaded.state === "loading") {
    return <p className="text-xs text-muted-foreground">Loading the preview…</p>;
  }
  if (loaded.state === "error") {
    return (
      <p className="text-xs text-muted-foreground" data-slot="file-text-error">
        The preview could not be loaded. You can still download the file.
      </p>
    );
  }
  return (
    <>
      <pre
        data-slot="file-text"
        // biome-ignore lint/a11y/noNoninteractiveTabindex: a scrollable region must be reachable by keyboard
        tabIndex={0}
        className="max-h-96 overflow-auto rounded-lg bg-muted/60 p-3 font-mono text-xs leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere]"
      >
        {loaded.text}
      </pre>
      {loaded.truncated ? (
        <p className="text-xs text-muted-foreground" data-slot="file-text-truncated">
          Showing the first {TEXT_PREVIEW_BYTES / 1024} KiB of {formatSize(file.size)}. Download the
          file to read all of it.
        </p>
      ) : null}
    </>
  );
}

const ICON = { image: ImageIcon, text: FileTextIcon } as const;

/** The card of a kept file, in the turn after the agent's words. */
export function KeptFileCard({ file }: { file: KeptFile }) {
  const title = fileTitle(file);
  const Icon = file.preview ? ICON[file.preview] : FileIcon;
  const details = [formatSize(file.size), file.mimeType].filter(Boolean).join(" · ");
  return (
    <div
      data-slot="file-card"
      data-preview={file.preview ?? "none"}
      className="flex w-full max-w-xl min-w-0 flex-col gap-2 rounded-xl border bg-card p-3.5 sm:p-4"
    >
      <div className="flex min-w-0 items-center gap-3">
        <span
          aria-hidden="true"
          className="flex size-9 shrink-0 items-center justify-center rounded-lg bg-muted text-muted-foreground"
        >
          <Icon className="size-4.5" />
        </span>
        <div className="min-w-0 flex-1">
          <p data-slot="file-name" className="text-sm font-medium [overflow-wrap:anywhere]">
            {title}
          </p>
          {details ? (
            <p data-slot="file-details" className="truncate text-xs text-muted-foreground">
              {details}
            </p>
          ) : null}
        </div>
        <a
          data-slot="file-download"
          aria-label={`Download ${title}`}
          href={downloadHref(file)}
          download
          className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-full border px-3 text-[0.8125rem] font-medium text-foreground no-underline transition-colors hover:bg-muted focus-visible:outline-offset-2"
        >
          <DownloadIcon aria-hidden="true" className="size-3.5" />
          Download
        </a>
      </div>
      {file.preview === "image" ? <FileImage file={file} /> : null}
      {file.preview === "text" ? <FileText file={file} /> : null}
    </div>
  );
}
