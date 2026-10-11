"use client";

import type { ReactNode } from "react";
import { useState } from "react";
import { DownloadControl } from "@/features/chat/components/cards/kept-file-card";
import { useObjectUrl } from "@/features/chat/hooks/use-object-url";
import { mustFetch } from "@/features/chat/lib/file-access";
import { type KeptFile, plainName } from "@/features/chat/lib/files";
import { useBrowserAuth } from "@/lib/auth/use-browser-auth";
import { useThreadFiles } from "./thread-files";

/*
 * The `Video` and `Audio` components of the UI catalog (docs/api/ui-catalog-v1.md, version 6, group 4): a video or a
 * sound recording of a file of this thread, named by its sha256, and played in the page with its controls. The file is
 * the thread's own route (`href`), never a URL the agent wrote: a link cannot carry the tokens of the browser mode, so
 * there the file is fetched and played from an object URL (`blob:`, allowed by the page's media policy). A file that
 * does not play says so, and keeps its download. The agent's title names it, and its caption is text.
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

function MediaFile({ kind, props }: { kind: "video" | "audio"; props: Rec }): ReactNode {
  const files = useThreadFiles();
  const auth = useBrowserAuth();
  const [failed, setFailed] = useState(false);
  const artifact = str(props.artifact);
  const file: KeptFile | undefined = files.find((f) => f.sha256 === artifact);
  // the kind is the validator's check too: a Video names a video, an Audio a sound (rule artifact)
  const right = file?.mimeType?.startsWith(kind === "video" ? "video/" : "audio/") === true;
  const fetched = file !== undefined && right && mustFetch(auth, file.href);
  const object = useObjectUrl(file?.href ?? "", fetched);
  const title = plainName(str(props.title) ?? "", 200);
  const caption = str(props.caption) ?? "";
  if (!file || !right) {
    return (
      <p
        data-slot={`${kind}-missing`}
        className="rounded-md border border-dashed px-2.5 py-2 text-xs text-muted-foreground"
      >
        {`This ${kind === "video" ? "video" : "sound"} is not one of this conversation's files.`}
      </p>
    );
  }
  const src = object.state === "ready" ? object.url : file.href;
  const label = title || (kind === "video" ? "Video" : "Sound");
  return (
    <figure data-slot={kind} className="m-0 flex min-w-0 flex-col gap-2">
      {kind === "video" ? (
        // biome-ignore lint/a11y/useMediaCaption: the agent gives no captions; the title and the caption are text
        <video
          controls
          playsInline
          preload="metadata"
          src={src}
          aria-label={label}
          onError={() => setFailed(true)}
          className="max-h-96 w-full rounded-lg bg-black"
        />
      ) : (
        // biome-ignore lint/a11y/useMediaCaption: the agent gives no transcript; the title and the caption are text
        <audio
          controls
          preload="metadata"
          src={src}
          aria-label={label}
          onError={() => setFailed(true)}
          className="w-full"
        />
      )}
      {failed ? (
        <p data-slot={`${kind}-error`} className="text-xs text-muted-foreground">
          {`This ${kind === "video" ? "video" : "sound"} could not be played. You can still download it.`}
        </p>
      ) : null}
      <figcaption className="flex min-w-0 items-center gap-2 text-sm">
        <span className="min-w-0 flex-1 font-medium [overflow-wrap:anywhere]">{label}</span>
        <DownloadControl file={file} title={label} fetched={fetched} />
      </figcaption>
      {caption ? (
        <p className="text-xs whitespace-pre-wrap text-muted-foreground [overflow-wrap:anywhere]">
          {caption}
        </p>
      ) : null}
    </figure>
  );
}

/** The Video of a surface; the props are those of the lowered component (`prepare.ts`). */
export function VideoFile(props: Rec): ReactNode {
  return <MediaFile kind="video" props={props} />;
}

/** The Audio of a surface; the props are those of the lowered component (`prepare.ts`). */
export function AudioFile(props: Rec): ReactNode {
  return <MediaFile kind="audio" props={props} />;
}
