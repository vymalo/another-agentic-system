/** The longest a client waits between two attempts to reopen a stream (ADR 0082). */
export const BACKOFF_CAP_MS = 30_000;

/**
 * How long to wait before reconnect attempt `attempt` (0-based), in ms.
 *
 * The growth the loop always had — 1 s, 2 s, 4 s… under the cap — cut by a uniform draw over the whole
 * window (full jitter). ADR 0082, C3 asks for "a growing, randomised wait": growth is what gets one
 * client past a restart, and the draw is what keeps a fleet a proxy cut at one instant from coming back
 * in one wave. `random` is the draw, injected so a test can say what the window is without waiting.
 */
export const backoffMs = (attempt: number, random: () => number = Math.random): number =>
  Math.floor(random() * Math.min(BACKOFF_CAP_MS, 1000 * 2 ** attempt));
