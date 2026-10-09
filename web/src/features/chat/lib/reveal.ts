import type { Connection } from "./agui/thread-agent";

/** What the page knows of how far an opened thread has come (`useChatRuntime`). */
export type Progress = {
  /** The log is caught up with the thread (sticky). */
  loaded: boolean;
  /** The stream delivered runs that the runtime does not show yet (`ThreadSnapshot.replaying`). */
  replaying: boolean;
  connection: Connection;
  /** The newest event the stream delivered; 0 when it delivered none. */
  lastSeq: number;
};

/**
 * Whether the transcript of an opened thread may be shown (ADR 0059, slice 1). A replay applies the log's runs one after
 * the other, each a render of a transcript that grows, and a transcript shown meanwhile is a page that fills in front of
 * the person and scrolls after its own bottom. So the page holds it back until the replay is applied: the log is caught
 * up and the runtime shows every run the stream delivered, and then shows it already at the end.
 *
 * A connection that is down with part of the log in shows that part (what the page did before it held anything back),
 * once the runtime has it; one that never delivered anything shows nothing, and the skeleton stays.
 */
export function isSettled({ loaded, replaying, connection, lastSeq }: Progress): boolean {
  if (replaying) return false;
  return loaded || (connection === "reconnecting" && lastSeq > 0);
}
