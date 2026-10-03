import type { SendMode } from "@/features/chat/lib/agui/vymalo";

/**
 * The words around sending while an agent works (ADR 0036). Pure, so the composer, the bubble's
 * note and the tests say the same thing.
 */

/** ADR 0036: an agent whose live card lists this reads a steered message in its running task. */
export const STEER_URI = "https://agents.vymalo.com/a2a/extensions/steer/v1";

/**
 * When a steered message is read: "at its next step" for an agent whose card lists `steer/v1`,
 * "after this turn" for any other, and for a card that could not be read (nothing is promised that
 * the card has not said: fail closed, ADR 0008).
 */
export const readsWhen = (steers: boolean | null): "at its next step" | "after this turn" =>
  steers === true ? "at its next step" : "after this turn";

/** The sentence under "Send" in the menu, and the button's description. */
export const sendHint = (agent: string, steers: boolean | null): string =>
  `${agent} reads it ${readsWhen(steers)}`;

export const stopHint = (agent: string): string =>
  `Stops ${agent} and starts again with your message`;

/**
 * The quiet line under a message that was sent while the agent worked. What it says is what
 * normally happens (the agent's card, read now); the order of the log is the truth.
 */
export const deliveryNote = (mode: SendMode, agent: string, steers: boolean | null): string =>
  mode === "interrupt"
    ? `Stopped ${agent} · it starts again from here`
    : `Sent while ${agent} was working · read ${readsWhen(steers)}`;

/** A message as the composer's keyboard sends it while a run is open: Enter, or with Ctrl/⌘+Shift. */
export function modeOfKey(e: {
  key: string;
  shiftKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey?: boolean;
}): SendMode | null {
  if (e.key !== "Enter" || e.altKey) return null;
  if (e.shiftKey) return e.ctrlKey || e.metaKey ? "interrupt" : null; // Shift+Enter is a new line
  return e.ctrlKey || e.metaKey ? null : "steer";
}
