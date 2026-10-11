import type { Tier } from "./tier";

/**
 * The tiers of the messages this page sent (ADR 0061). A message this page sends is not heard back
 * from the stream (a run does not hear its own message), so its tier is kept by the message's id for
 * the bubble's badge to draw; one that comes from the log (a reload, another tab) carries its tier in
 * its metadata (`vymalo.tier`) instead.
 */
export class SentTiers {
  private readonly sent = new Map<string, Tier>();
  private readonly listeners = new Set<() => void>();
  private version = 0;

  /** A message went out under `tier`. */
  remember(messageId: string, tier: Tier): void {
    this.sent.set(messageId, tier);
    this.changed();
  }

  /** The send was refused: the message is not in the transcript, so its tier is not its. */
  forget(messageId: string): void {
    if (this.sent.delete(messageId)) this.changed();
  }

  /** The tier of a message this page sent, by its id. */
  of(messageId: string): Tier | undefined {
    return this.sent.get(messageId);
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** Changes whenever a sent tier does. */
  getVersion = (): number => this.version;

  private changed(): void {
    this.version++;
    for (const listener of [...this.listeners]) listener();
  }
}
