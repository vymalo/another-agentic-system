import type { ApiAgent } from "@/lib/api/types";
import { forTrimmed, type Mention, reconcile, standing } from "./mentions";

/**
 * The mentions of the message in the box, and of the messages this page sent.
 *
 * The box's text is assistant-ui's (`composer.text`); the mentions are ours, kept beside it and
 * moved with every edit of it (`sync`, which the composer calls whenever the text changes). A
 * message does not leave through us: the runtime sends it, clears the box, and `ThreadAgent` posts
 * the run. So the agent asks here for what the text it is about to post mentions (`take`), and the
 * answer does not depend on whether the box was already cleared: what was in the box when it was
 * emptied is kept until the run is accepted (`accepted`), and given back to the box when the send
 * was refused and the runtime put the text back (`sync` with the same text), so a refused message
 * keeps its mentions.
 *
 * A message this page sent is not heard back from the stream (a run does not hear its own message),
 * so its mentions are kept by the message's id for the bubble to draw (`sentOf`); one that comes
 * from the log (a reload, another tab) carries them in its metadata instead.
 */
export class MentionsStore {
  private text = "";
  private items: Mention[] = [];
  /** What was in the box when it was emptied by a send, until the run is accepted or the box is written in again. */
  private gone: { text: string; items: Mention[] } | null = null;
  private readonly sent = new Map<string, Mention[]>();
  private readonly listeners = new Set<() => void>();
  private version = 0;

  /** The mentions of the text in the box, in order. */
  get current(): readonly Mention[] {
    return this.items;
  }

  /** The box's text changed (typing, a paste, a pick, the runtime clearing or restoring it). */
  sync(text: string): void {
    if (text === this.text) return;
    if (text === "") {
      if (this.items.length > 0) this.gone = { text: this.text, items: this.items };
      this.items = [];
    } else if (this.gone && this.text === "" && this.gone.text.trim() === text.trim()) {
      // the text came back after a refused send: so do its mentions (the same text, moved past what was trimmed)
      this.items = reconcile(this.gone.text, text, this.gone.items);
      this.gone = null;
    } else {
      this.items = reconcile(this.text, text, this.items);
      this.gone = null;
    }
    this.text = text;
    this.changed();
  }

  /**
   * The text and its mentions at once: a pick, which writes both. The text is already the box's, so
   * the next `sync` with it is no change.
   */
  set(text: string, items: readonly Mention[]): void {
    this.text = text;
    this.items = [...items];
    this.gone = null;
    this.changed();
  }

  /**
   * A refused message whose text goes back in front of what was written since (`${text}\n\n${rest}`):
   * its mentions come back with it, and the new text's after them.
   */
  restoreInFront(sentText: string, sentItems: readonly Mention[], rest: string): string {
    const text = rest.trim() ? `${sentText}\n\n${rest}` : sentText;
    const lead = sentText.length + 2;
    const behind = rest.trim()
      ? this.items.map((m) => ({ ...m, start: m.start + lead, end: m.end + lead }))
      : [];
    this.text = text;
    this.items = [...sentItems, ...behind];
    this.gone = null;
    this.changed();
    return text;
  }

  /** A pick may carry a card URL the list has since changed: the box's mentions follow the list. */
  followList(agents: readonly ApiAgent[]): void {
    let moved = false;
    const next = this.items.map((m) => {
      const card = agents.find((a) => a.id === m.agentId)?.cardUrl;
      if (card === undefined || card === m.cardUrl) return m;
      moved = true;
      return { ...m, cardUrl: card };
    });
    if (!moved) return;
    this.items = next;
    this.changed();
  }

  /**
   * The mentions of the message that is going out as `message`: those of the box (or, once it was
   * emptied by the send, of what it held), moved to the text as it goes out. With a `messageId` they
   * are kept for the bubble. Nothing when the message is not what the box held.
   */
  take(message: string, messageId?: string): Mention[] {
    const from = this.gone && this.text === "" ? this.gone : { text: this.text, items: this.items };
    const carried =
      from.text === message
        ? standing(message, from.items)
        : from.text.trim() === message
          ? forTrimmed(from.text, from.items)
          : [];
    if (messageId !== undefined && carried.length > 0) {
      this.sent.set(messageId, carried);
      this.changed();
    }
    return carried;
  }

  /** The run was accepted: what the box held is the log's now. */
  accepted(): void {
    this.gone = null;
  }

  /** The send was refused: the message is not in the transcript, so its mentions are not its. */
  refused(messageId: string): void {
    if (this.sent.delete(messageId)) this.changed();
  }

  /** The mentions of a message this page sent, by its id. */
  sentOf(messageId: string): readonly Mention[] | undefined {
    return this.sent.get(messageId);
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** Changes whenever the box's mentions or the sent ones do. */
  getVersion = (): number => this.version;

  private changed(): void {
    this.version++;
    for (const listener of [...this.listeners]) listener();
  }
}
