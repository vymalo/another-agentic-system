/**
 * The mock's copy of the live overlay of the orchestrator's AG-UI projection
 * (`orch-agui-projection`, `src/live.rs`; docs/api/agui.md "Live text", ADR 0027): the words of a
 * reply that is still being written, as frames made beside the projection of the log, never with an
 * `id:`, and merged by message id with the log's own message. `golden.test.ts` plays the `stream`
 * golden through it, so it cannot drift from the real one unnoticed.
 *
 * The rules, as in the real overlay: a piece for a message the log has said, or one that closed a
 * moment ago, is late; with no invocation open it is held (32 at most) until the next logged event
 * opens one; a message opens at a piece that starts at offset 0 and grows by the part of each piece
 * beyond what was said; the log's message for the open id continues it (`CONTENT` with the rest and
 * `final`, `END` with `final` and its resume point); an open message ends as given up when the
 * stream gives up, when another opens, or before its invocation or the run closes; the log's
 * message for an id that was given up is said under `<id>~final`.
 *
 * Offsets are UTF-16 code units here. (On the wire between the orchestrator's processes they are
 * UTF-8 bytes; the frames a screen reads count in UTF-16, which is what this makes.)
 */
import { actorMetaOf, type Frame, type Projector } from "./projection";

/** One piece of a reply as the sender relays it. */
export type LivePiece = {
  /** The id of the stream, which is the id of the log's message that completes it. */
  messageId: string;
  /** The agent that writes it (the invocation that is open must be its own). */
  agent: string;
  /** UTF-16 code units before `text` in the whole text. */
  offset: number;
  text: string;
  /** `last`: the final piece (changes nothing: the log's message closes the live one); `abandoned`: given up. */
  end: "open" | "last" | "abandoned";
};

const HELD_MAX = 32;
const CLOSED_MEMORY = 64;
const KEY = "vymalo.live";

type Ev = Frame["event"];
type Rewrite =
  | { kind: "merge"; id: string; sent: string }
  | { kind: "rename"; from: string; to: string };

export class LiveOverlay {
  private open: { id: string; sub: string; sent: string } | null = null;
  private readonly closed: { id: string; how: "merged" | "abandoned" }[] = [];
  private held: LivePiece[] = [];

  /** The id of the live message that is open, if any. */
  get openMessage(): string | undefined {
    return this.open?.id;
  }

  /**
   * The frames the projector made for one log event (call after `Projector.apply`): the same
   * frames, except that the log's message for the open live message continues it and that an open
   * live message ends as given up before the frame that closes its invocation or the run.
   */
  logged(projector: Projector, frames: Frame[]): Frame[] {
    const out: Frame[] = [];
    let rewrite: Rewrite | undefined;
    for (const frame of frames) {
      const e = frame.event;
      const id = typeof e.messageId === "string" ? e.messageId : "";
      if (e.type === "TEXT_MESSAGE_START") {
        if (this.open?.id === id) {
          rewrite = { kind: "merge", id, sent: this.open.sent };
          this.remember(id, "merged");
          this.open = null;
          continue; // the live message is open on the wire already
        }
        if (this.was(id, "abandoned")) {
          rewrite = { kind: "rename", from: id, to: `${id}~final` };
          out.push({ ...frame, event: { ...e, messageId: `${id}~final` } });
          continue;
        }
      } else if (e.type === "TEXT_MESSAGE_CONTENT" && rewrite) {
        if (rewrite.kind === "merge" && id === rewrite.id) {
          const delta = typeof e.delta === "string" ? e.delta : "";
          const continues = delta.startsWith(rewrite.sent);
          out.push({
            ...frame,
            event: {
              ...e,
              delta: continues ? delta.slice(rewrite.sent.length) : delta,
              metadata: {
                [KEY]: { offset: continues ? rewrite.sent.length : 0, final: true },
              },
            },
          });
          continue;
        }
        if (rewrite.kind === "rename" && id === rewrite.from) {
          out.push({ ...frame, event: { ...e, messageId: rewrite.to } });
          continue;
        }
      } else if (e.type === "TEXT_MESSAGE_END" && rewrite) {
        if (rewrite.kind === "merge" && id === rewrite.id) {
          out.push({ ...frame, event: { ...e, metadata: { [KEY]: { final: true } } } });
          rewrite = undefined;
          continue;
        }
        if (rewrite.kind === "rename" && id === rewrite.from) {
          out.push({ ...frame, event: { ...e, messageId: rewrite.to } });
          rewrite = undefined;
          continue;
        }
      } else if (e.type === "SUBAGENT_FINISHED" || e.type === "SUBAGENT_ERROR") {
        if (this.open && e.subagentRunId === this.open.sub) this.abandon(out);
      } else if (e.type === "RUN_FINISHED" || e.type === "RUN_ERROR") {
        this.abandon(out);
        this.held = [];
      }
      out.push(frame);
    }
    if (this.held.length > 0 && projector.runOpen && projector.openInvocation()) {
      const held = this.held;
      this.held = [];
      for (const piece of held) this.accept(projector, piece, out);
    }
    return out;
  }

  /** One piece of live text: the frames to write now, none of them a resume point. */
  live(projector: Projector, piece: LivePiece): Frame[] {
    const out: Frame[] = [];
    this.accept(projector, piece, out);
    return out;
  }

  private accept(projector: Projector, piece: LivePiece, out: Frame[]) {
    const id = piece.messageId;
    if (id === "" || projector.hasMessage(id) || this.closed.some((c) => c.id === id)) return;
    if (!projector.runOpen) return;
    const inv = projector.openInvocation();
    if (!inv) {
      if (this.held.length === HELD_MAX) this.held.shift();
      this.held.push(piece);
      return;
    }
    if (inv.name !== piece.agent) return;
    if (this.open?.id !== id) {
      const opens = piece.offset === 0 && piece.text !== "" && piece.end !== "abandoned";
      if (!opens) {
        if (piece.end === "abandoned") this.remember(id, "abandoned");
        return;
      }
      this.abandon(out); // another stream begins: the one that was open is over
      out.push({
        event: {
          type: "TEXT_MESSAGE_START",
          messageId: id,
          role: "assistant",
          name: inv.name,
          subagentRunId: inv.id,
          metadata: { ...actorMetaOf(inv.actor), [KEY]: {} },
        },
      });
      this.open = { id, sub: inv.id, sent: "" };
    }
    const open = this.open;
    if (open && piece.offset <= open.sent.length) {
      const rest = piece.text.slice(open.sent.length - piece.offset);
      if (rest !== "") {
        out.push({
          event: {
            type: "TEXT_MESSAGE_CONTENT",
            messageId: open.id,
            delta: rest,
            subagentRunId: open.sub,
            metadata: { [KEY]: { offset: open.sent.length } },
          },
        });
        open.sent += rest;
      }
    }
    if (piece.end === "abandoned") this.abandon(out);
  }

  private abandon(out: Frame[]) {
    const open = this.open;
    if (!open) return;
    this.open = null;
    const end: Ev = {
      type: "TEXT_MESSAGE_END",
      messageId: open.id,
      subagentRunId: open.sub,
      metadata: { [KEY]: { abandoned: true } },
    };
    out.push({ event: end });
    this.remember(open.id, "abandoned");
  }

  private remember(id: string, how: "merged" | "abandoned") {
    if (this.closed.length === CLOSED_MEMORY) this.closed.shift();
    this.closed.push({ id, how });
  }

  private was(id: string, how: "merged" | "abandoned"): boolean {
    return this.closed.some((c) => c.id === id && c.how === how);
  }
}
