import {
  ACTIVITY,
  type ArtifactContent,
  activityPartName,
  type FilePreview,
  parseArtifact,
} from "@/features/chat/lib/agui/vymalo";

/*
 * The files an agent handed over and the artifact store kept (ADR 0032, docs/api/agui.md "Typed
 * artifacts"), as the web reads them. A file is fetched from its `href` (the API's route for the
 * thread's files) and from nothing else: never from the agent's `uri`. An image is drawn as an
 * `<img src>` (never inline SVG markup), a text file as text, and every file can be downloaded.
 * Pure: no React, no I/O but `readTextPreview`'s fetch, which the caller hands in.
 */

/** A file the store kept, with what the web needs to show it. */
export type KeptFile = {
  sha256: string;
  /** Where the API serves it: `/api/threads/<thread>/artifacts/<sha256>`. */
  href: string;
  size: number;
  /** The agent's file name (untrusted), when it gave one. */
  filename?: string;
  /** The artifact's own name (untrusted): the fallback title. */
  name: string;
  /** The type the worker sniffed. */
  mimeType?: string;
  preview: FilePreview | null;
};

/** The alt text of an image that has no file name to say. */
export const GENERIC_ALT = "File from the agent";

/** The kept file an artifact is, or undefined (a link, a pull request, a file that was not kept). */
export function keptFileOf(artifact: ArtifactContent): KeptFile | undefined {
  if (artifact.kind !== "file") return undefined;
  const { href, sha256, size } = artifact;
  if (href === undefined || sha256 === undefined || size === undefined) return undefined;
  return {
    sha256,
    href,
    size,
    name: artifact.name,
    preview: artifact.preview ?? null,
    ...(artifact.filename !== undefined ? { filename: artifact.filename } : {}),
    ...(artifact.mimeType !== undefined ? { mimeType: artifact.mimeType } : {}),
  };
}

/** `?download=1` makes the API answer with `Content-Disposition: attachment`. */
export const downloadHref = (file: Pick<KeptFile, "href">): string => `${file.href}?download=1`;

// biome-ignore lint/suspicious/noControlCharactersInRegex: stripping them is the point
const UNSAFE = /[\u0000-\u001f\u007f-\u009f‎‏‪-‮⁦-⁩]/g;
const MAX_NAME = 120;

/** A text from an agent, as one short line: no control or direction characters, at most `max`. */
export function plainName(name: string, max = MAX_NAME): string {
  const clean = name.replace(UNSAFE, "").trim();
  return clean.length > max ? `${clean.slice(0, max - 1)}…` : clean;
}

/** What a file is called: its file name, else the artifact's name, else a generic word. */
export function fileTitle(file: Pick<KeptFile, "filename" | "name">): string {
  return plainName(file.filename ?? "") || plainName(file.name) || "File";
}

/** The alt text of a file's image: its file name, else a generic sentence. */
export function fileAlt(file: Pick<KeptFile, "filename">): string {
  return plainName(file.filename ?? "") || GENERIC_ALT;
}

/** `2.0 KB`, `12 bytes`: a size a person reads (powers of 1024, one decimal above a byte). */
export function formatSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return "";
  if (bytes < 1024) return bytes === 1 ? "1 byte" : `${bytes} bytes`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(value >= 100 ? 0 : 1)} ${units[unit]}`;
}

type PartLike = { type: string; name?: string; data?: unknown };
export type FileMessage = { role: string; content: readonly PartLike[] };

const ARTIFACT_PART = activityPartName(ACTIVITY.artifact);

/** Every file the thread holds, in the order the agents handed them over, each hash once. */
export function filesOf(messages: readonly FileMessage[]): KeptFile[] {
  const found = new Map<string, KeptFile>();
  for (const message of messages) {
    if (message.role !== "assistant") continue;
    for (const part of message.content) {
      if (part.type !== "data" || part.name !== ARTIFACT_PART) continue;
      const artifact = parseArtifact(part.data);
      const file = artifact ? keptFileOf(artifact) : undefined;
      if (file && !found.has(file.sha256)) found.set(file.sha256, file);
    }
  }
  return [...found.values()];
}

/** Whether two lists name the same files (what a hook keeps the earlier list for). */
export function sameFiles(a: readonly KeptFile[], b: readonly KeptFile[]): boolean {
  return (
    a.length === b.length &&
    a.every((f, i) => f.sha256 === b[i]?.sha256 && f.href === b[i]?.href && f.size === b[i]?.size)
  );
}

/** How much of a text file is read for its preview. */
export const TEXT_PREVIEW_BYTES = 64 * 1024;

export type TextPreview = { text: string; truncated: boolean };

/**
 * The first `TEXT_PREVIEW_BYTES` of a text file, decoded as UTF-8 (an invalid sequence is a
 * replacement character, never an error). The rest of a longer file is not downloaded: the stream
 * is cancelled. Throws when the API does not answer 200. The text is drawn as text by the caller.
 */
export async function readTextPreview(
  href: string,
  fetcher: typeof fetch = fetch,
  signal?: AbortSignal,
): Promise<TextPreview> {
  const res = await fetcher(href, {
    credentials: "same-origin",
    ...(signal ? { signal } : {}),
  });
  if (!res.ok) throw new Error(`the file could not be read (${res.status})`);
  const decoder = new TextDecoder("utf-8", { fatal: false });
  if (!res.body) {
    const bytes = new Uint8Array(await res.arrayBuffer());
    const truncated = bytes.length > TEXT_PREVIEW_BYTES;
    return { text: decoder.decode(bytes.slice(0, TEXT_PREVIEW_BYTES)), truncated };
  }
  const reader = res.body.getReader();
  let text = "";
  let read = 0;
  let truncated = false;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    const room = TEXT_PREVIEW_BYTES - read;
    if (value.length > room) {
      text += decoder.decode(value.slice(0, room));
      truncated = true;
      await reader.cancel();
      break;
    }
    read += value.length;
    text += decoder.decode(value, { stream: true });
  }
  return { text: text + decoder.decode(), truncated };
}
