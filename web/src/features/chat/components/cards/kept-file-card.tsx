"use client";

import { DownloadIcon, FileIcon, FileTextIcon, ImageIcon, ImageOffIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { useObjectUrl } from "@/features/chat/hooks/use-object-url";
import { fetchFileBlob, mustFetch, saveBlob } from "@/features/chat/lib/file-access";
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
import { apiFetch } from "@/lib/api/client";
import { useBrowserAuth } from "@/lib/auth/use-browser-auth";

/*
 * A file the agent made and the artifact store kept (ADR 0032): its name, its size, a download, and
 * a preview when the type has one. Everything about the file is the agent's text and drawn as text.
 * The bytes are fetched from the file's `href` and from nothing else: an image is an `<img src>`
 * (never inline markup, so an SVG runs nothing and loads nothing), a text file is read (the first
 * 64 KiB) and drawn in a `<pre>`, and any other type is the card alone. Where the web holds its own
 * tokens (ADR 0054) a link cannot carry them: the image is fetched with DPoP and shown from an object
 * URL, the text is read through the same session, and the download is a fetch that is saved; a
 * public share link's files stay plain links (`lib/file-access.ts`).
 */

/**
 * A picture of the file; if the browser cannot decode it, the card says so and keeps the download.
 * `inline` is a picture in the agent's words: it sits in a paragraph, so its lines are spans, and
 * the file has no card of its own to download from.
 */
export function FileImage({
  file,
  alt,
  className,
  inline = false,
}: {
  file: KeptFile;
  /** What the picture says in words; the file's name when the agent gave no better. */
  alt?: string | undefined;
  className?: string;
  inline?: boolean;
}) {
  const [broken, setBroken] = useState(false);
  const fetched = mustFetch(useBrowserAuth(), file.href);
  const object = useObjectUrl(file.href, fetched);
  const Line = inline ? "span" : "p";
  if (broken || object.state === "error") {
    return (
      <Line
        data-slot="file-image-error"
        className="flex items-center gap-2 rounded-md border border-dashed px-2.5 py-2 text-xs text-muted-foreground"
      >
        <ImageOffIcon aria-hidden="true" className="size-4 shrink-0" />
        <span>
          {inline
            ? "The image could not be shown."
            : "The image could not be shown. You can still download it."}
        </span>
      </Line>
    );
  }
  if (fetched && object.state !== "ready") {
    return (
      <Line data-slot="file-image-loading" className="block text-xs text-muted-foreground">
        Loading the image…
      </Line>
    );
  }
  return (
    // biome-ignore lint/performance/noImgElement: a same-origin file the API serves; next/image would proxy it
    <img
      data-slot="file-image"
      src={object.state === "ready" ? object.url : file.href}
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
  const fetched = mustFetch(useBrowserAuth(), file.href);
  useEffect(() => {
    const controller = new AbortController();
    setLoaded({ state: "loading" });
    readTextPreview(file.href, fetched ? apiFetch : fetch, controller.signal).then(
      (preview) => setLoaded({ state: "ready", ...preview }),
      () => {
        if (!controller.signal.aborted) setLoaded({ state: "error" });
      },
    );
    return () => controller.abort();
  }, [file.href, fetched]);

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

const DOWNLOAD_CLASS =
  "inline-flex h-8 shrink-0 cursor-pointer items-center gap-1.5 rounded-full border px-3 text-[0.8125rem] font-medium text-foreground no-underline transition-colors hover:bg-muted focus-visible:outline-offset-2";

/** A link with `download` for a file the cookie can fetch; a button that fetches it with DPoP and saves it where the web holds its own tokens. */
function DownloadControl({
  file,
  title,
  fetched,
}: {
  file: KeptFile;
  title: string;
  fetched: boolean;
}) {
  const [failed, setFailed] = useState(false);
  const content = (
    <>
      <DownloadIcon aria-hidden="true" className="size-3.5" />
      Download
    </>
  );
  if (!fetched) {
    return (
      <a
        data-slot="file-download"
        aria-label={`Download ${title}`}
        href={downloadHref(file)}
        download
        className={DOWNLOAD_CLASS}
      >
        {content}
      </a>
    );
  }
  return (
    <button
      type="button"
      data-slot="file-download"
      aria-label={failed ? `Download ${title} (it failed, try again)` : `Download ${title}`}
      onClick={() => {
        setFailed(false);
        fetchFileBlob(downloadHref(file)).then(
          (blob) => saveBlob(blob, title),
          () => setFailed(true),
        );
      }}
      className={DOWNLOAD_CLASS}
    >
      {content}
    </button>
  );
}

const ICON = { image: ImageIcon, text: FileTextIcon } as const;

/** The card of a kept file, in the turn after the agent's words. */
export function KeptFileCard({ file }: { file: KeptFile }) {
  const title = fileTitle(file);
  const fetched = mustFetch(useBrowserAuth(), file.href);
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
        <DownloadControl file={file} title={title} fetched={fetched} />
      </div>
      {file.preview === "image" ? <FileImage file={file} /> : null}
      {file.preview === "text" ? <FileText file={file} /> : null}
    </div>
  );
}
