/**
 * What a person is told when a subagent is refused. The contract names a code for each refusal
 * (`docs/api/chat-api.yaml`: `startChildSession`, `closeChildSession`, `runAgent` on a child thread);
 * the words here are the screen's own, in the screen's vocabulary (a helper session is a *subagent*).
 *
 * A code this module does not know falls back to the problem's `detail`, so a refusal added to the
 * contract later still reaches the person in the server's words. The agent's own words (the `reason` of
 * a start that failed) are text of the agent's and never pass through here.
 */

const WORDS = {
  child_thread: "A subagent cannot start another subagent.",
  no_child_sessions: "This agent does not offer subagents right now.",
  unknown_helper: "This agent has no subagent with that name.",
  bad_description: "Describe the subagent in one line of 1 to 80 characters.",
  no_context:
    "Send the agent a message first: a subagent starts from a conversation the agent has begun.",
  too_many_starts: "Wait for the subagents you started to open, then try again.",
  too_many_sessions: "This thread has reached the most subagents it can keep.",
  not_child_thread: "This thread is not a subagent.",
  session_closed: "This subagent is closed and takes no more messages.",
  child_mentions: "A subagent cannot ask other agents, so a message to it cannot mention them.",
  session_closes_on_stop:
    "This subagent closes when its work is stopped, so Stop & send is not available. Send the message when it is idle, or start it kept open.",
} as const satisfies Record<string, string>;

export type RefusalCode = keyof typeof WORDS;

/** Every code the contract names for a refused subagent request. */
export const REFUSAL_CODES = Object.keys(WORDS) as RefusalCode[];

const known = (code: string): code is RefusalCode => Object.hasOwn(WORDS, code);

/** The words for a refusal: ours for a code we know, else the server's `detail`, else a plain sentence. */
export function refusalWords(code: string | undefined, detail: string | undefined): string {
  if (code !== undefined && known(code)) return WORDS[code];
  if (detail !== undefined && detail.trim() !== "") return detail;
  return "The request was refused. Try again.";
}
