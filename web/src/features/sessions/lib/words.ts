import type { SessionCard } from "./cards";

/*
 * What a session card says, in words (never colour alone). The agent's own words (the helper's name, the
 * description, a start's reason) are not here: the components draw them as text.
 */

/** The state of a session in words: Working, Waiting for you, Idle, Closed, or the round that failed. */
export function sessionStateWords(card: SessionCard): string {
  if (card.state === "closed") return "Closed";
  if (card.report === "failed")
    return card.round === undefined ? "Round failed" : `Round ${card.round} failed`;
  if (card.state === "working") return card.report === "blocked" ? "Waiting for you" : "Working";
  return "Idle";
}

/** The words of a subagent's row: closing wins over what the round says, closed over everything. */
export function subagentStateWords(card: SessionCard): string {
  if (card.state === "closed") return "Closed";
  if (card.closing || card.closeAsked) return "Closing…";
  return sessionStateWords(card);
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The route of a child thread, made only of a UUID; anything else is no link. */
export function threadHref(threadId: string | undefined): string | undefined {
  return threadId !== undefined && UUID.test(threadId) ? `/threads/${threadId}` : undefined;
}

/** A card the client marked as the one to draw (`ThreadAgent` sets `home` in the run that is its home). */
export function isHomeCard(content: unknown): boolean {
  return (
    typeof content === "object" && content !== null && (content as { home?: unknown }).home === true
  );
}

/** Why a thread cannot be forked yet: whether it is a subagent's own is not known until it is read. */
export const THREAD_NOT_KNOWN = "Loading this thread";
/** Why a subagent's own thread is not forked, shared or handed to another agent: it is part of its parent chat. */
export const CHILD_PART_OF_PARENT = "A subagent is part of its parent chat";
