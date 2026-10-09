"use client";

import { useAuiState } from "@assistant-ui/react";
import { useMemo } from "react";
import type { KeptFile } from "@/features/chat/lib/files";
import {
  fileSource,
  hashesOf,
  imageSourcesIn,
  resolveSharedImage,
  type SharedMessage,
  sharedFilesOf,
  type Turn,
} from "@/features/chat/lib/inline-images";

/** The message this hook is under: which turn is asking, and where it is in the thread. */
function useTurn(): Turn {
  const run = useAuiState((s) => s.message.id);
  const index = useAuiState((s) => s.message.index);
  return useMemo(() => ({ run, index }), [run, index]);
}

const NO_MESSAGES: readonly SharedMessage[] = [];
const NO_HASHES: ReadonlySet<string> = new Set();

/**
 * The kept file an image in an agent's words means (`lib/inline-images.ts`), or undefined: the source
 * is not a path, the message is not the agent's, or nothing the thread holds is that image. The
 * thread's messages are read only for an image that could mean a file, so a text with none costs
 * nothing per streamed word.
 */
export function useSharedImage(src: string | undefined): KeptFile | undefined {
  const turn = useTurn();
  const agent = useAuiState((s) => s.message.role === "assistant");
  const wanted = agent && src !== undefined && fileSource(src) !== undefined;
  const messages = useAuiState((s) =>
    wanted ? (s.thread.messages as readonly SharedMessage[]) : NO_MESSAGES,
  );
  return useMemo(
    () =>
      wanted && src !== undefined
        ? resolveSharedImage(src, sharedFilesOf(messages), turn)
        : undefined,
    [wanted, src, messages, turn],
  );
}

/**
 * The hashes of the files the words of this turn draw inline: the turn's list of files leaves them
 * out, because the picture in the words is the file. Empty without an image in the words. The text is
 * parsed when it changes (a word of a reply being written) and not again for a thread update that
 * leaves it as it was; the files it means are looked up when either changes.
 */
export function useInlineHashes(markdown: string): ReadonlySet<string> {
  const turn = useTurn();
  const sources = useMemo(() => imageSourcesIn(markdown), [markdown]);
  const wanted = sources.length > 0;
  const messages = useAuiState((s) =>
    wanted ? (s.thread.messages as readonly SharedMessage[]) : NO_MESSAGES,
  );
  return useMemo(
    () => (wanted ? hashesOf(sources, sharedFilesOf(messages), turn) : NO_HASHES),
    [wanted, sources, messages, turn],
  );
}
