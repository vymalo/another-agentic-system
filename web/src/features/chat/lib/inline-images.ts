import { markdownRefs, refUrl } from "@/lib/markdown-refs";
import { ACTIVITY, activityPartName, parseArtifact, parseStep } from "./agui/vymalo";
import { type KeptFile, keptFileOf } from "./files";

/*
 * Images in an agent's words that mean a file the agent shared (the owner's thread of 2026-10-09):
 * the coder took screenshots, shared each with `share_file` and then wrote
 * `![Matches](shots/4-matches.png)`. The path means nothing to a browser, so it is looked up among
 * the files the thread holds, and only there: the answer is a kept file's own `href` or nothing, and
 * a source is never a URL the page requests (ADR 0032: the agent's `uri` is never fetched; remote
 * images in Markdown stay refused). Pure: no React, no I/O.
 */

/** A kept file, with the path of the worktree it was shared from when a step said so. */
export type SharedFile = {
  file: KeptFile;
  /** The share step's `input.path`: what the agent will call the file in its words. */
  path?: string;
  /** The message (a run of the agent) that shared it, and where that message is in the thread. */
  run: string;
  index: number;
};

/** The turn that asks: its message, and its place in the thread. */
export type Turn = { run: string; index: number };

type PartLike = { type: string; name?: string; data?: unknown };
export type SharedMessage = { id: string; role: string; content: readonly PartLike[] };

const STEP_PART = activityPartName(ACTIVITY.step);
const ARTIFACT_PART = activityPartName(ACTIVITY.artifact);

const baseName = (path: string): string => path.slice(path.lastIndexOf("/") + 1);

/** A step that named a worktree path (a share, a write, a read): what `share_file` is called with. */
type Offer = { at: number; path: string; name: string; claimed: boolean };

/** A kept file of a message, where in it, and the step that shared it once that is known. */
type Entry = { at: number; file: KeptFile; offer?: Offer };

/**
 * Whether a step could be the one that shared this file: its name for it (`input.name`, else the base
 * of `input.path`) is the artifact's name, or its path ends in the file's own name.
 */
const sharedBy = (offer: Offer, file: KeptFile): boolean =>
  offer.name === file.name ||
  (file.filename !== undefined && baseName(offer.path) === file.filename);

const cache = new WeakMap<readonly SharedMessage[], readonly SharedFile[]>();

/**
 * Every file the agents shared in the thread, in the order the log has them, once per share (a file
 * shared twice is two entries: the later one is the one a path means). The path comes from the step
 * that shared it, paired by name within the same message; a file whose step is missing (an agent
 * that is not adam, an input the job had no room for) has none and is found by its name. The list
 * is kept for the array it was made from: a turn with many images asks once per update.
 */
export function sharedFilesOf(messages: readonly SharedMessage[]): readonly SharedFile[] {
  const hit = cache.get(messages);
  if (hit) return hit;
  const found: SharedFile[] = [];
  for (const [index, message] of messages.entries()) {
    if (message.role !== "assistant") continue;
    const steps = new Map<string, Offer>();
    const kept: Entry[] = [];
    for (const [at, part] of message.content.entries()) {
      if (part.type !== "data") continue;
      if (part.name === STEP_PART) {
        const step = parseStep(part.data);
        const path = step?.input?.path;
        if (step && typeof path === "string" && path !== "") {
          const given = step.input?.name;
          // a step reported again keeps its place and takes the later report's input
          steps.set(step.id, {
            at: steps.get(step.id)?.at ?? at,
            path,
            name: typeof given === "string" && given !== "" ? given : baseName(path),
            claimed: false,
          });
        }
      } else if (part.name === ARTIFACT_PART) {
        const artifact = parseArtifact(part.data);
        const file = artifact ? keptFileOf(artifact) : undefined;
        if (file) kept.push({ at, file });
      }
    }
    // Each file takes the step before it that shared it, the nearest one that is not taken. Only then do
    // the files left over look after them, for a step that was first reported when it ended: were this one
    // pass, a file with no step before it (its input too big to keep) would take the next share's step.
    const offers = [...steps.values()];
    const take = (entry: Entry, offer: Offer | undefined) => {
      if (!offer) return;
      offer.claimed = true;
      entry.offer = offer;
    };
    for (const entry of kept) {
      take(
        entry,
        latest(offers, (o) => o.at < entry.at && !o.claimed && sharedBy(o, entry.file)),
      );
    }
    for (const entry of kept) {
      if (entry.offer) continue;
      take(
        entry,
        offers.find((o) => o.at > entry.at && !o.claimed && sharedBy(o, entry.file)),
      );
    }
    for (const { file, offer } of kept) {
      found.push({ file, run: message.id, index, ...(offer ? { path: offer.path } : {}) });
    }
  }
  cache.set(messages, found);
  return found;
}

const SCHEME = /^[a-z][a-z0-9+.-]*:/i;

/**
 * A path as the chat and the log both write it, so that two spellings of one file compare equal: without
 * a query or a fragment, percent-escapes read (the renderer's `normalizeUri` escapes spaces and anything
 * that is not ASCII in an image's source), and without a leading `./`. The step's path and the image's
 * source both go through it. A `?` or `#` in a real file name ends the path here, which leaves such a file
 * to the match by its file name.
 */
export function normalisePath(raw: string): string {
  let path = raw.trim().split(/[?#]/, 1)[0] ?? "";
  try {
    path = decodeURIComponent(path);
  } catch {
    // not valid percent-encoding: read as it is
  }
  while (path.startsWith("./")) path = path.slice(2);
  return path;
}

/**
 * The path an image's source names, when it names one: a relative reference (`shots/4.png`,
 * `./4.png`, `/work/demo/4.png`), normalised (`normalisePath`). Nothing for a source that names a
 * place: a scheme (`https:`, `data:`, `file:`, a drive letter) or another host (`//host/…`). Those are
 * not looked up and not requested.
 */
export function fileSource(src: string): string | undefined {
  const bare = src.trim();
  if (bare === "" || SCHEME.test(bare) || bare.startsWith("//")) return undefined;
  const path = normalisePath(bare);
  return path === "" ? undefined : path;
}

function latest<T>(items: readonly T[], test: (item: T) => boolean): T | undefined {
  for (let i = items.length - 1; i >= 0; i--) {
    const item = items[i] as T;
    if (test(item)) return item;
  }
  return undefined;
}

/**
 * The kept file an image of an answer means, or undefined. Only an image can be one, and only one
 * shared by the turn `turn` is, or by an earlier one (a turn cannot show what came after it). The
 * turn's own files first, then the rest: the file whose share step had exactly this path, else the
 * one whose file name (or name) is the source's base name; where a name was shared more than once
 * the latest share wins.
 */
export function resolveSharedImage(
  src: string,
  shared: readonly SharedFile[],
  turn?: Turn,
): KeptFile | undefined {
  const wanted = fileSource(src);
  if (wanted === undefined) return undefined;
  const name = baseName(wanted);
  if (name === "") return undefined;
  const images = shared.filter(
    (s) => s.file.preview === "image" && (turn === undefined || s.index <= turn.index),
  );
  const tiers =
    turn === undefined
      ? [images]
      : [images.filter((s) => s.run === turn.run), images.filter((s) => s.run !== turn.run)];
  for (const tier of tiers) {
    const hit =
      latest(tier, (s) => s.path !== undefined && normalisePath(s.path) === wanted) ??
      latest(tier, (s) => s.file.filename === name || s.file.name === name);
    if (hit) return hit.file;
  }
  return undefined;
}

/**
 * The sources of the images of a Markdown text, in order: `![alt](src)` and `![alt][ref]` with its
 * definition. Parsed, so an image written inside code is no image (it is not one in the chat).
 */
export function imageSourcesIn(markdown: string): string[] {
  if (!markdown.includes("![")) return [];
  const { refs, definitions } = markdownRefs(markdown);
  return refs.flatMap((ref) => {
    const src = ref.kind === "image" ? refUrl(ref, definitions) : undefined;
    return src === undefined ? [] : [src];
  });
}

/**
 * The hashes of the files these image sources mean: what an answer draws inline. The turn's list of
 * files leaves these out, because the picture in the words is the file; a file no image names stays.
 */
export function hashesOf(
  sources: readonly string[],
  shared: readonly SharedFile[],
  turn?: Turn,
): ReadonlySet<string> {
  const hashes = new Set<string>();
  for (const src of sources) {
    const file = resolveSharedImage(src, shared, turn);
    if (file) hashes.add(file.sha256);
  }
  return hashes;
}

/** `hashesOf` the images of a Markdown text. */
export function inlineFileHashes(
  markdown: string,
  shared: readonly SharedFile[],
  turn?: Turn,
): ReadonlySet<string> {
  return hashesOf(imageSourcesIn(markdown), shared, turn);
}
