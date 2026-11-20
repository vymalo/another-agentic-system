/** The part of a transcript message that decides whether a turn can be regenerated. */
export type Prior = {
  role: string;
  content: readonly { type: string; text?: string | undefined }[];
  metadata?: { custom?: Record<string, unknown> | undefined } | undefined;
};

/** What Regenerate says again: the person's message, by its place in the log, and its words. */
export type Began = { seq: number; text: string };

/**
 * The message Regenerate would say again, for the newest turn, or nothing when it must not be offered.
 * A turn begins with a message of the person (`prior`, the transcript message right before the agent's);
 * a turn that began with a form answer has none, the answer is an action in the agent's own message.
 * Not offered when:
 * - the message has no known place in the log (`seq`);
 * - it mentioned an agent: the edit route carries no mentions, so the new version would lose them;
 * - the thread waits for the answer to a question (`waiting`): the edit would drop the question;
 * - it was sent while the agent worked (`delivery`, steer or interrupt): the turn is more than that
 *   message (the agent's earlier work stays before it), so replacing only it is not "again".
 */
export function regenerableFrom(input: {
  prior: Prior | undefined;
  seq: number | undefined;
  mentioned: boolean;
  waiting: boolean;
}): Began | undefined {
  const { prior, seq, mentioned, waiting } = input;
  if (prior?.role !== "user" || seq === undefined || mentioned || waiting) return undefined;
  if (prior.metadata?.custom?.delivery !== undefined) return undefined;
  const text = prior.content
    .flatMap((p) => (p.type === "text" && p.text ? [p.text] : []))
    .join("\n\n");
  return text.trim() === "" ? undefined : { seq, text };
}
